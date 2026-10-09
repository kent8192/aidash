//! Community Provider Credential Store: independent transactions and pinned AEAD reads.
mod crypto;
use super::super::models::credential_store::{RegisteredKey, Resource, Version};
use aidash_application::{
	Error, Result, provider_access::KeyMaterialReader, provider_credentials::Store,
};
use async_trait::async_trait;
use chrono::Utc;
use crypto::{ALGORITHM, Key, version_aad};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, IntoIden, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use secrecy::{ExposeSecret, SecretString};
use std::collections::BTreeMap;
use uuid::Uuid;

pub struct PostgresStore {
	pool: crate::database::native::Pool,
	current: String,
	keys: BTreeMap<String, Key>,
}
pub(crate) struct RecoveryInventory {
	pub versions: Vec<(String, i64)>,
	pub keys: Vec<String>,
}
fn db_error(_: impl std::fmt::Display) -> Error {
	tracing::error!(
		reason = "database operation failed",
		"Provider Credential Store operation failed"
	);
	Error::External("Provider Credential Store is unavailable".into())
}
fn invalid(reason: &str) -> Error {
	Error::Invalid(reason.into())
}
async fn lock(tx: &mut dyn TransactionExecutor, identity: &str) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![Expr::value(identity).into(), Expr::value(157001_i64).into()],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values))
		.await
		.map_err(db_error)?;
	Ok(())
}
async fn delete_version(
	tx: &mut dyn TransactionExecutor,
	resource: &str,
	version: i64,
) -> Result<()> {
	// The pinned Manager primary-key delete binds only one column. Build the
	// composite predicate with the ORM QuerySet, then use the native executor.
	let query = Version::objects()
		.filter(Version::field_resource().eq(resource))
		.filter(Version::field_version().eq(version))
		.delete_query()
		.map_err(db_error)?;
	let (sql, values) = query.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values))
		.await
		.map_err(db_error)?;
	Ok(())
}
fn resource_id(resource: &str) -> Result<()> {
	let id = resource
		.strip_prefix("aidash-store/provider-credentials/")
		.and_then(|s| Uuid::parse_str(s).ok())
		.filter(|id| id.get_version_num() == 7);
	if id.is_none() {
		return Err(invalid("invalid Provider Credential Store resource"));
	}
	Ok(())
}
fn version_id(value: &str) -> Result<(&str, i64)> {
	let (resource, number) = value
		.rsplit_once("/versions/")
		.ok_or_else(|| invalid("invalid Provider Credential Store version"))?;
	resource_id(resource)?;
	let n = number
		.parse::<i64>()
		.ok()
		.filter(|n| *n > 0 && n.to_string() == number)
		.ok_or_else(|| invalid("invalid Provider Credential Store version"))?;
	Ok((resource, n))
}
impl PostgresStore {
	pub async fn new(
		pool: crate::database::native::Pool,
		current: SecretString,
		retired: Vec<SecretString>,
	) -> Result<Self> {
		let store = Self::from_keys(pool, current, retired)?;
		store.verify_registry().await?;
		Ok(store)
	}
	fn from_keys(
		pool: crate::database::native::Pool,
		current: SecretString,
		retired: Vec<SecretString>,
	) -> Result<Self> {
		let key = Key::parse(&current).map_err(|e| invalid(&e.to_string()))?;
		let current = key.id.clone();
		let mut keys = BTreeMap::from([(current.clone(), key)]);
		for value in retired {
			let key = Key::parse(&value).map_err(|e| invalid(&e.to_string()))?;
			keys.insert(key.id.clone(), key);
		}
		Ok(Self {
			pool,
			current,
			keys,
		})
	}
	/// Offline recovery checks known keys without registering a replacement key.
	pub(crate) async fn for_recovery(
		pool: crate::database::native::Pool,
		current: SecretString,
		retired: Vec<SecretString>,
	) -> Result<Self> {
		let store = Self::from_keys(pool, current, retired)?;
		let mut tx = store.begin().await?;
		let registered = RegisteredKey::objects()
			.all()
			.all_with_executor(tx.as_mut())
			.await
			.map_err(db_error)?;
		store.verify_configured_keys(&registered)?;
		tx.commit().await.map_err(db_error)?;
		Ok(store)
	}
	fn verify_configured_keys(&self, registered: &[RegisteredKey]) -> Result<()> {
		for row in registered {
			if let Some(key) = self.keys.get(&row.key_id) {
				key.verify(&row.check_nonce, &row.check_ciphertext)
					.map_err(|_| invalid("Provider Credential Store Master Key check failed"))?;
			}
		}
		Ok(())
	}
	pub(crate) async fn recovery_inventory(&self) -> Result<RecoveryInventory> {
		let mut tx = self.begin().await?;
		// Recovery inspects references only, never encrypted version payloads.
		let (sql, values) = Query::select()
			.columns(["resource", "version", "key_id"].map(Alias::new))
			.from(Alias::new("credential_store_versions"))
			.build(PostgresQueryBuilder);
		let mut versions = Vec::new();
		for row in tx
			.fetch_all(&sql, convert_values(values))
			.await
			.map_err(db_error)?
		{
			let key_id: String = row.get("key_id").map_err(db_error)?;
			if !self.keys.contains_key(&key_id) {
				let resource: String = row.get("resource").map_err(db_error)?;
				// Each supported namespace gets its own lifecycle adapter; #159 adds Plan Tokens.
				resource_id(&resource)?;
				versions.push((resource, row.get("version").map_err(db_error)?));
			}
		}
		let (sql, values) = Query::select()
			.column(Alias::new("key_id"))
			.from(Alias::new("credential_store_keys"))
			.build(PostgresQueryBuilder);
		let mut keys = Vec::new();
		for row in tx
			.fetch_all(&sql, convert_values(values))
			.await
			.map_err(db_error)?
		{
			let key_id: String = row.get("key_id").map_err(db_error)?;
			if !self.keys.contains_key(&key_id) {
				keys.push(key_id);
			}
		}
		tx.commit().await.map_err(db_error)?;
		Ok(RecoveryInventory { versions, keys })
	}
	pub(crate) async fn remove_unavailable(&self, inventory: &RecoveryInventory) -> Result<()> {
		let mut tx = self.begin().await?;
		for (resource, version) in &inventory.versions {
			let rows = Version::objects()
				.filter(Version::field_resource().eq(resource))
				.filter(Version::field_version().eq(*version))
				.select_for_update()
				.all_with_executor(tx.as_mut())
				.await
				.map_err(db_error)?;
			if let Some(row) = rows.first()
				&& !self.keys.contains_key(&row.key_id)
			{
				delete_version(tx.as_mut(), resource, *version).await?;
			}
		}
		for key_id in &inventory.keys {
			if !self.keys.contains_key(key_id) {
				RegisteredKey::objects()
					.delete_with_executor(tx.as_mut(), key_id.clone())
					.await
					.map_err(db_error)?;
			}
		}
		tx.commit().await.map_err(db_error)
	}
	async fn begin(&self) -> Result<Box<dyn TransactionExecutor>> {
		self.pool.connection().begin().await.map_err(db_error)
	}
	async fn verify_registry(&self) -> Result<()> {
		let mut tx = self.begin().await?;
		// Serialize the empty-registry decision as well as concurrent first boot/rotation.
		lock(
			tx.as_mut(),
			"aidash-provider-credential-store/key-registry/v1",
		)
		.await?;
		let registered = RegisteredKey::objects()
			.all()
			.all_with_executor(tx.as_mut())
			.await
			.map_err(db_error)?;
		if !registered.is_empty() && !registered.iter().any(|r| self.keys.contains_key(&r.key_id)) {
			return Err(invalid(
				"Provider Credential Store Master Key does not match any registered key identifier; stop server and worker and use manage provider-credential-store-recovery with the node settings",
			));
		}
		self.verify_configured_keys(&registered)?;
		let key = &self.keys[&self.current];
		let (nonce, ciphertext) = key
			.check()
			.map_err(|_| invalid("Provider Credential Store key check encryption failed"))?;
		let (sql, values) = Query::insert()
			.into_table(Alias::new("credential_store_keys"))
			.columns([
				Alias::new("key_id"),
				Alias::new("check_nonce"),
				Alias::new("check_ciphertext"),
				Alias::new("created_at"),
			])
			.from_subquery(
				Query::select()
					.expr(Expr::value(key.id.clone()))
					.expr(Expr::value(nonce))
					.expr(Expr::value(ciphertext))
					.expr(Expr::value(Utc::now()))
					.to_owned(),
			)
			.on_conflict(
				OnConflict::column(Alias::new("key_id"))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values))
			.await
			.map_err(db_error)?;
		let stored = RegisteredKey::objects()
			.filter(RegisteredKey::field_key_id().eq(&self.current))
			.all_with_executor(tx.as_mut())
			.await
			.map_err(db_error)?;
		let stored = stored
			.first()
			.ok_or_else(|| invalid("Provider Credential Store key registration failed"))?;
		key.verify(&stored.check_nonce, &stored.check_ciphertext)
			.map_err(|_| invalid("Provider Credential Store Master Key check failed"))?;

		// Count unavailable key identifiers without loading every encrypted payload.
		let (sql, values) = Query::select()
			.column(Alias::new("key_id"))
			.expr_as(
				reinhardt::query::Func::count(Expr::col(Alias::new("resource")).into()),
				Alias::new("count"),
			)
			.from(Alias::new("credential_store_versions"))
			.group_by_col(Alias::new("key_id"))
			.build(PostgresQueryBuilder);
		let rows = tx
			.fetch_all(&sql, convert_values(values))
			.await
			.map_err(db_error)?;
		let mut unavailable = BTreeMap::<String, i64>::new();
		for row in rows {
			let key_id = row.get::<String>("key_id").map_err(db_error)?;
			if !self.keys.contains_key(&key_id) {
				unavailable.insert(key_id, row.get::<i64>("count").map_err(db_error)?);
			}
		}
		tx.commit().await.map_err(db_error)?;
		for (key_id, count) in unavailable {
			tracing::warn!(
				key_id,
				count,
				"Provider Credential Store versions use an unconfigured Master Key"
			);
		}
		Ok(())
	}
	async fn read_pinned(
		&self,
		tenant: &str,
		resource: &str,
		version: &str,
	) -> std::result::Result<SecretString, &'static str> {
		resource_id(resource).map_err(|_| "invalid resource")?;
		let (pinned, n) = version_id(version).map_err(|_| "invalid version")?;
		if pinned != resource {
			return Err("resource mismatch");
		}
		let mut tx = self.begin().await.map_err(|_| "database unavailable")?;
		let rows = Version::objects()
			.filter(Version::field_resource().eq(resource))
			.filter(Version::field_version().eq(n))
			.all_with_executor(tx.as_mut())
			.await
			.map_err(|_| "database unavailable")?;
		let row = rows.first().ok_or("missing version")?;
		if row.tenant != tenant {
			return Err("Tenant mismatch");
		}
		if row.state != "enabled" {
			return Err("disabled version");
		}
		if row.algorithm != ALGORITHM {
			return Err("unsupported algorithm");
		}
		let key = self.keys.get(&row.key_id).ok_or("unknown key identifier")?;
		let plaintext = key
			.open(
				&row.nonce,
				&row.ciphertext,
				&version_aad(tenant, resource, n),
			)
			.map_err(|_| "decryption failure")?;
		let value = std::str::from_utf8(&plaintext).map_err(|_| "invalid plaintext encoding")?;
		let secret = SecretString::from(value.to_owned());
		tx.commit().await.map_err(|_| "database unavailable")?;
		Ok(secret)
	}
}
#[async_trait]
impl KeyMaterialReader for PostgresStore {
	async fn read(&self, tenant: &str, resource: &str, version: &str) -> Result<SecretString> {
		self.read_pinned(tenant, resource, version)
			.await
			.map_err(|reason| {
				tracing::warn!(
					reason,
					"Provider Credential Store cannot read the pinned version"
				);
				invalid("Provider Credential Store cannot read the pinned version")
			})
	}
}
#[async_trait]
impl Store for PostgresStore {
	fn resource(&self, id: Uuid) -> String {
		format!("aidash-store/provider-credentials/{id}")
	}
	async fn create(&self, tenant: &str, id: Uuid) -> Result<()> {
		aidash_domain::policy::identifier(tenant)?;
		let resource = self.resource(id);
		resource_id(&resource)?;
		let mut tx = self.begin().await?;
		let row = Resource::build()
			.resource(resource)
			.tenant(tenant.to_owned())
			.next_version(1)
			.created_at(Utc::now())
			.finish();
		Resource::objects()
			.insert_with_executor(tx.as_mut(), &row)
			.await
			.map_err(db_error)?;
		tx.commit().await.map_err(db_error)
	}
	async fn add_version(
		&self,
		tenant: &str,
		resource: &str,
		key_material: &SecretString,
	) -> Result<String> {
		resource_id(resource)?;
		let mut tx = self.begin().await?;
		let rows = Resource::objects()
			.filter(Resource::field_resource().eq(resource))
			.select_for_update()
			.all_with_executor(tx.as_mut())
			.await
			.map_err(db_error)?;
		let mut row = rows
			.into_iter()
			.next()
			.ok_or_else(|| invalid("Provider Credential Store resource is missing"))?;
		if row.tenant != tenant {
			return Err(invalid("Provider Credential Store Tenant mismatch"));
		}
		let n = row.next_version;
		row.next_version = n
			.checked_add(1)
			.ok_or_else(|| invalid("Provider Credential Store version counter exhausted"))?;
		Resource::objects()
			.save_with_executor(tx.as_mut(), &row)
			.await
			.map_err(db_error)?;
		let (nonce, ciphertext) = self.keys[&self.current]
			.seal(
				key_material.expose_secret().as_bytes(),
				&version_aad(tenant, resource, n),
			)
			.map_err(|_| invalid("Provider Credential Store encryption failed"))?;
		let row = Version::build()
			.resource(resource.to_owned())
			.version(n)
			.tenant(tenant.to_owned())
			.key_id(self.current.clone())
			.algorithm(ALGORITHM)
			.nonce(nonce)
			.ciphertext(ciphertext)
			.state("enabled")
			.created_at(Utc::now())
			.disabled_at(None)
			.finish();
		Version::objects()
			.insert_with_executor(tx.as_mut(), &row)
			.await
			.map_err(db_error)?;
		tx.commit().await.map_err(db_error)?;
		Ok(format!("{resource}/versions/{n}"))
	}
	async fn versions(&self, resource: &str) -> Result<Vec<String>> {
		resource_id(resource)?;
		let mut tx = self.begin().await?;
		let rows = Version::objects()
			.filter(Version::field_resource().eq(resource))
			.order_by(&["version"])
			.all_with_executor(tx.as_mut())
			.await
			.map_err(db_error)?;
		tx.commit().await.map_err(db_error)?;
		Ok(rows
			.into_iter()
			.map(|v| format!("{resource}/versions/{}", v.version))
			.collect())
	}
	async fn disable(&self, version: &str) -> Result<()> {
		let (resource, n) = version_id(version)?;
		let mut tx = self.begin().await?;
		let rows = Version::objects()
			.filter(Version::field_resource().eq(resource))
			.filter(Version::field_version().eq(n))
			.select_for_update()
			.all_with_executor(tx.as_mut())
			.await
			.map_err(db_error)?;
		if let Some(mut row) = rows.into_iter().next()
			&& row.state != "disabled"
		{
			row.state = "disabled".into();
			row.disabled_at = Some(Utc::now());
			Version::objects()
				.save_with_executor(tx.as_mut(), &row)
				.await
				.map_err(db_error)?;
		}
		tx.commit().await.map_err(db_error)
	}
	async fn destroy(&self, version: &str) -> Result<()> {
		let (resource, n) = version_id(version)?;
		let mut tx = self.begin().await?;
		delete_version(tx.as_mut(), resource, n).await?;
		tx.commit().await.map_err(db_error)
	}
	async fn delete(&self, resource: &str) -> Result<()> {
		resource_id(resource)?;
		let mut tx = self.begin().await?;
		Resource::objects()
			.delete_with_executor(tx.as_mut(), resource.to_owned())
			.await
			.map_err(db_error)?;
		tx.commit().await.map_err(db_error)
	}
}
