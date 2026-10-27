//! OAuth persistence. Owner scopes serialize grant writes and refreshes across processes.
use super::super::models::oauth_connections::{
	OAuthAuthorization, OAuthConnection as Record, OAuthDynamicClient,
};
use aidash_application::{
	Error, Result,
	oauth_connections::{DynamicClient, Owner, Repository, Scope},
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::oauth_connections::OAuthConnection;
use async_trait::async_trait;
use reinhardt::{
	db::{
		backends::TransactionExecutor,
		orm::{Model, execution::convert_values},
	},
	query::{
		Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
		SimpleExpr,
	},
};
use uuid::Uuid;

/// Service subjects, remote Runs and calls without a durable Run grant have no person owner.
pub async fn owner_of_run(
	store: &crate::store::Store,
	run: &aidash_domain::Run,
) -> Result<Option<Owner>> {
	if !run.is_local(store.deployment.as_str()) {
		return Ok(None);
	}
	use aidash_application::ports::authorization::ExecutionGrantRepository;
	let grants = super::execution::Grants { store };
	let Some(grant) = grants.grant(run.id).await? else {
		return Ok(None);
	};
	let snapshot = crate::apps::identity::services::core::Authorization {
		pool: store.pool.clone(),
	}
	.snapshot(&grant.tenant)
	.await?;
	if !super::approval_preferences::is_person(&snapshot.bundle, &grant.root_subject) {
		return Ok(None);
	}
	Ok(Some(Owner {
		tenant: grant.tenant,
		subject: grant.root_subject,
	}))
}

#[derive(Clone)]
pub struct NativeRepository {
	pub pool: crate::database::native::Pool,
	pub execution: crate::database::native::Pool,
}
struct NativeScope {
	tx: Box<dyn TransactionExecutor>,
	owner: Owner,
}
fn framework(e: impl Into<reinhardt::core::exception::Error>) -> Error {
	crate::Error::from(e.into()).into()
}
fn contract(r: Record) -> Result<OAuthConnection> {
	Ok(serde_json::from_value(serde_json::to_value(r)?)?)
}
async fn lock(tx: &mut dyn TransactionExecutor, key: String, namespace: i64) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![SimpleExpr::FunctionCall(
				"hashtextextended".into_iden(),
				vec![Expr::value(key).into(), Expr::value(namespace).into()],
			)],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?;
	Ok(())
}
#[async_trait]
impl Repository for NativeRepository {
	async fn begin(&self, owner: &Owner) -> Result<Box<dyn Scope>> {
		if owner.subject.is_empty() || owner.subject.len() > 256 {
			return Err(Error::Invalid("invalid OAuth Connection owner".into()));
		}
		let mut tx = self
			.pool
			.begin(&TenancyScope::tenant(&owner.tenant)?)
			.await?
			.into_executor()
			.await?;
		lock(
			tx.as_mut(),
			serde_json::json!([owner.tenant, owner.subject]).to_string(),
			554_001,
		)
		.await?;
		Ok(Box::new(NativeScope {
			tx,
			owner: owner.clone(),
		}))
	}
	async fn owners(&self, tenant: &str) -> Result<Vec<Owner>> {
		let mut tx = self
			.pool
			.begin(&TenancyScope::tenant(tenant)?)
			.await?
			.into_executor()
			.await?;
		let mut owners: Vec<_> = Record::objects()
			.filter(Record::field_tenant().eq(tenant))
			.all_with_executor(tx.as_mut())
			.await
			.map_err(framework)?
			.into_iter()
			.map(|r| r.subject)
			.collect();
		// Include pending-only owners so retirement cancels every callback.
		owners.extend(
			OAuthAuthorization::objects()
				.filter(OAuthAuthorization::field_tenant().eq(tenant))
				.all_with_executor(tx.as_mut())
				.await
				.map_err(framework)?
				.into_iter()
				.map(|r| r.subject),
		);
		tx.commit().await.map_err(crate::Error::from)?;
		owners.sort();
		owners.dedup();
		Ok(owners
			.into_iter()
			.map(|subject| Owner {
				tenant: tenant.into(),
				subject,
			})
			.collect())
	}
	async fn dynamic_client(
		&self,
		tenant: &str,
		issuer: &str,
		redirect_uri: &str,
	) -> Result<Option<DynamicClient>> {
		let mut tx = self
			.pool
			.begin(&TenancyScope::tenant(tenant)?)
			.await?
			.into_executor()
			.await?;
		let row = cached(tx.as_mut(), tenant, issuer, redirect_uri).await?;
		tx.commit().await.map_err(crate::Error::from)?;
		Ok(row.map(dynamic))
	}
	async fn cache_client(&self, c: &DynamicClient) -> Result<DynamicClient> {
		let mut tx = self
			.pool
			.begin(&TenancyScope::tenant(&c.tenant)?)
			.await?
			.into_executor()
			.await?;
		lock(
			tx.as_mut(),
			serde_json::json!([c.tenant, c.issuer, c.redirect_uri]).to_string(),
			554_003,
		)
		.await?;
		let winner =
			if let Some(r) = cached(tx.as_mut(), &c.tenant, &c.issuer, &c.redirect_uri).await? {
				dynamic(r)
			} else {
				let record = OAuthDynamicClient::build()
					.id(c.id)
					.tenant(c.tenant.clone())
					.issuer(c.issuer.clone())
					.redirect_uri(c.redirect_uri.clone())
					.client_id(c.client_id.clone())
					.secret_resource(c.secret_resource.clone())
					.pinned_version(c.pinned_version.clone())
					.finish();
				OAuthDynamicClient::objects()
					.insert_with_executor(tx.as_mut(), &record)
					.await
					.map_err(framework)?;
				c.clone()
			};
		tx.commit().await.map_err(crate::Error::from)?;
		Ok(winner)
	}
	async fn dynamic_clients(&self, tenant: &str) -> Result<Vec<DynamicClient>> {
		let mut tx = self
			.pool
			.begin(&TenancyScope::tenant(tenant)?)
			.await?
			.into_executor()
			.await?;
		let rows = OAuthDynamicClient::objects()
			.filter(OAuthDynamicClient::field_tenant().eq(tenant))
			.all_with_executor(tx.as_mut())
			.await
			.map_err(framework)?;
		tx.commit().await.map_err(crate::Error::from)?;
		Ok(rows.into_iter().map(dynamic).collect())
	}
	async fn delete_dynamic_client(&self, tenant: &str, id: Uuid) -> Result<()> {
		let mut tx = self
			.pool
			.begin(&TenancyScope::tenant(tenant)?)
			.await?
			.into_executor()
			.await?;
		let query = OAuthDynamicClient::objects()
			.filter(OAuthDynamicClient::field_tenant().eq(tenant))
			.filter(OAuthDynamicClient::field_id().eq(id))
			.delete_query()
			.map_err(framework)?;
		let (sql, values) = query.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		tx.commit().await.map_err(crate::Error::from)?;
		Ok(())
	}
	async fn answer_requests(&self, owner: &Owner, resource: &str) -> Result<()> {
		Ok(answer_pending(
			&self.pool,
			&self.execution,
			owner,
			aidash_domain::oauth_connections::CONNECTION_REQUEST_KIND,
			resource,
		)
		.await?)
	}
}
async fn cached(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	issuer: &str,
	redirect: &str,
) -> Result<Option<OAuthDynamicClient>> {
	Ok(OAuthDynamicClient::objects()
		.filter(OAuthDynamicClient::field_tenant().eq(tenant))
		.filter(OAuthDynamicClient::field_issuer().eq(issuer))
		.filter(OAuthDynamicClient::field_redirect_uri().eq(redirect))
		.all_with_executor(tx)
		.await
		.map_err(framework)?
		.pop())
}
fn dynamic(r: OAuthDynamicClient) -> DynamicClient {
	DynamicClient {
		id: r.id,
		tenant: r.tenant,
		issuer: r.issuer,
		redirect_uri: r.redirect_uri,
		client_id: r.client_id,
		secret_resource: r.secret_resource,
		pinned_version: r.pinned_version,
	}
}
#[async_trait]
impl Scope for NativeScope {
	async fn list(&mut self) -> Result<Vec<OAuthConnection>> {
		Record::objects()
			.filter(Record::field_tenant().eq(self.owner.tenant.clone()))
			.filter(Record::field_subject().eq(self.owner.subject.clone()))
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(framework)?
			.into_iter()
			.map(contract)
			.collect()
	}
	async fn save(&mut self, row: &OAuthConnection) -> Result<()> {
		if row.tenant != self.owner.tenant || row.subject != self.owner.subject {
			return Err(Error::Forbidden);
		}
		let exists = self.list().await?.iter().any(|r| r.id == row.id);
		let r: Record = serde_json::from_value(serde_json::to_value(row)?)?;
		if exists {
			Record::objects()
				.save_with_executor(self.tx.as_mut(), &r)
				.await
				.map_err(framework)?;
		} else {
			Record::objects()
				.insert_with_executor(self.tx.as_mut(), &r)
				.await
				.map_err(framework)?;
		}
		Ok(())
	}
	async fn delete(&mut self, id: Uuid) -> Result<()> {
		if !self.list().await?.iter().any(|r| r.id == id) {
			return Err(Error::Forbidden);
		}
		Record::objects()
			.delete_with_executor(self.tx.as_mut(), id)
			.await
			.map_err(framework)?;
		Ok(())
	}
	async fn cancel_attempts(&mut self, resource: Option<&str>) -> Result<()> {
		let mut query = Query::delete();
		query
			.from_table(Alias::new("oauth_authorizations"))
			.and_where(Expr::col("tenant").eq(Expr::value(self.owner.tenant.clone())))
			.and_where(Expr::col("subject").eq(Expr::value(self.owner.subject.clone())));
		if let Some(resource) = resource {
			query.and_where(Expr::col("resource").eq(Expr::value(resource)));
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		self.tx
			.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Ok(self.tx.commit().await.map_err(crate::Error::from)?)
	}
}
/// W3b replaces this policy at composition with its clients/ceilings repository.
pub struct UnconfiguredPolicy;
#[async_trait]
impl aidash_application::ports::oauth::ConnectionPolicy for UnconfiguredPolicy {
	async fn registered_clients(
		&self,
		_: &str,
		_: &str,
		_: &str,
	) -> Result<(
		Option<aidash_domain::oauth_connections::OAuthClient>,
		Option<aidash_domain::oauth_connections::OAuthClient>,
	)> {
		Ok((None, None))
	}
	async fn scope_ceiling(&self, _: &str, _: &str) -> Result<Option<Vec<String>>> {
		Ok(None)
	}
}

impl OAuthAuthorization {
	pub(crate) async fn reserve(&self, pool: &crate::database::native::Pool) -> crate::Result<()> {
		let scope = TenancyScope::Deployment(DeploymentScopeReason::OAuthAuthorization);
		pool.atomic(&scope, async |tx| {
			lock(tx, "pending".into(), 554_002).await?;
			let now = super::super::models::database_time(tx).await?;
			Self::objects()
				.filter(Self::field_expires_at().lte(now))
				.delete_with_conn(tx)
				.await?;
			Self::objects()
				.filter(Self::field_browser_hash().eq(self.browser_hash.clone()))
				.filter(Self::field_session_id().eq(self.session_id))
				.filter(Self::field_tenant().eq(self.tenant.clone()))
				.filter(Self::field_resource().eq(self.resource.clone()))
				.delete_with_conn(tx)
				.await?;
			if Self::objects().all().count_with_db(tx).await? >= 10_000
				|| Self::objects()
					.filter(Self::field_browser_hash().eq(self.browser_hash.clone()))
					.count_with_db(tx)
					.await? >= 8
			{
				return Err(crate::Error::Conflict(
					"too many pending OAuth authorizations".into(),
				));
			}
			Self::objects().create_with_conn(tx, self).await?;
			Ok(())
		})
		.await
	}
	pub(crate) async fn consume(
		pool: &crate::database::native::Pool,
		state_hash: String,
	) -> crate::Result<Self> {
		let scope = TenancyScope::Deployment(DeploymentScopeReason::OAuthAuthorization);
		let (row, now) = pool
			.atomic(&scope, async |tx| {
				let row = Self::objects()
					.filter(Self::field_state_hash().eq(state_hash))
					.select_for_update()
					.all_with_executor(tx)
					.await
					.map_err(reinhardt::core::exception::Error::from)?
					.pop()
					.ok_or(crate::Error::Unauthorized)?;
				Self::objects()
					.filter(Self::field_state_hash().eq(row.state_hash.clone()))
					.delete_with_conn(tx)
					.await?;
				let now = super::super::models::database_time(tx).await?;
				Ok((row, now))
			})
			.await?;
		if row.expires_at <= now {
			return Err(crate::Error::Unauthorized);
		}
		Ok(row)
	}
	pub(crate) fn pending(self) -> aidash_application::oauth_connections::Pending {
		aidash_application::oauth_connections::Pending {
			owner_tenant: self.tenant,
			owner_subject: self.subject,
			resource: self.resource,
			issuer: self.issuer,
			client: self.client.into_inner(),
			scopes: self.scopes,
			pkce_verifier: self.pkce_verifier,
			redirect_uri: self.redirect_uri,
			return_to: self.return_to,
			connection: self.connection,
			revision: self.revision,
		}
	}
}
/// Generic answer path does not decode the request-kind enum (owned by W5).
/// Both stores are scoped to the same Tenant; root ownership comes from the
/// authoritative control grant, including child Runs.
pub async fn answer_pending(
	control: &crate::database::native::Pool,
	execution: &crate::database::native::Pool,
	owner: &Owner,
	kind: &str,
	resource: &str,
) -> crate::Result<()> {
	use crate::apps::identity::models::AuthorizationExecution;
	use reinhardt::db::orm::connection::QueryRow;
	let scope = TenancyScope::tenant(&owner.tenant)?;
	let mut control_tx = control.begin(&scope).await?.into_executor().await?;
	let grants = AuthorizationExecution::objects()
		.filter(AuthorizationExecution::field_tenant().eq(owner.tenant.clone()))
		.filter(AuthorizationExecution::field_root_subject().eq(owner.subject.clone()))
		.order_by(&["run_id"])
		.all_with_executor(control_tx.as_mut())
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	control_tx.commit().await?;
	let mut tx = execution.begin(&scope).await?.into_executor().await?;
	for grant in grants {
		let (sql, values) = Query::select()
			.columns(["id", "prompt"].map(Alias::new))
			.from(Alias::new("human_requests"))
			.and_where(Expr::col("tenant").eq(Expr::value(owner.tenant.clone())))
			.and_where(Expr::col("run_id").eq(Expr::value(grant.run_id)))
			.and_where(Expr::col("kind").eq(Expr::value(kind)))
			.and_where(Expr::col("response").is_null())
			.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
			.lock(reinhardt::query::LockType::Update)
			.build(PostgresQueryBuilder);
		for row in tx.fetch_all(&sql, convert_values(values)).await? {
			let value = QueryRow::from_backend_row(row).data;
			let payload: serde_json::Value =
				serde_json::from_str(value["prompt"].as_str().unwrap_or_default())
					.unwrap_or_default();
			if payload["resource"].as_str() != Some(resource) {
				continue;
			}
			let id: Uuid = serde_json::from_value(value["id"].clone())?;
			let response = serde_json::json!({"connected":true});
			let (sql, values) = Query::update()
				.table(Alias::new("human_requests"))
				.value_expr(Alias::new("response"), Expr::value(response.clone()))
				.value_expr(
					Alias::new("answered_by"),
					Expr::value(owner.subject.clone()),
				)
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.and_where(Expr::col("response").is_null())
				.returning_all()
				.build(PostgresQueryBuilder);
			let answered = tx.fetch_one(&sql, convert_values(values)).await?;
			crate::apps::execution::models::event_records::append_run(
				tx.as_mut(),
				grant.run_id,
				Some(grant.workspace_id),
				"human.answered",
				QueryRow::from_backend_row(answered).data,
			)
			.await?;
		}
	}
	tx.commit().await?;
	Ok(())
}

/// Resolves each Registry reference in a short read transaction. OAuth
/// discovery and consent preparation must not keep a Registry connection open.
pub(crate) struct ToolLookup {
	pub pool: crate::database::native::Pool,
	pub tenant: String,
}
#[async_trait]
impl aidash_application::ports::registry::DefinitionLookup for ToolLookup {
	async fn definition(
		&mut self,
		id: &str,
		version: &str,
	) -> Result<aidash_domain::registry::Entry> {
		let mut tx = self
			.pool
			.begin(&TenancyScope::tenant(&self.tenant)?)
			.await?
			.into_executor()
			.await?;
		let value = aidash_application::ports::registry::DefinitionLookup::definition(
			&mut crate::apps::registry::repositories::NativeScope(tx.as_mut()),
			id,
			version,
		)
		.await?;
		tx.commit().await.map_err(crate::Error::from)?;
		Ok(value)
	}
}
