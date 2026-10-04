//! Native transfer adapters preserve snapshot identity, receipt row locks and retry predicates.
use super::capability_records::{domain, native};
use crate::apps::execution::capabilities::{
	serializers::{
		contracts::{Area as NativeArea, FileEntry},
		records::Record as NativeRecord,
		transfer::{Description as NativeDescription, Identity as NativeIdentity},
	},
	services::{objects::PendingObject, records, sessions},
};
use crate::{
	Error as NativeError, Result as NativeResult,
	authorization::{access::Access, catalog, identity::SubjectIdentity, peer},
	domain::Run,
	federation::Federation,
	registry::{EntityRef, Entry},
	store::Store,
};
use aidash_application::{
	Error, Result,
	ports::capabilities::transfer::{Limits, ReceiptScope, TransferRepository, TransferScope},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{
		operations::MountedFile,
		records::Record,
		sessions::Area,
		transfer::{Description, Identity},
	},
	policy::Resource,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) federation: &'a Federation,
}
pub(crate) enum Authority<'a> {
	Owned(Box<Access>),
	Borrowed(&'a mut Access),
}
impl Authority<'_> {
	pub(crate) fn get(&self) -> &Access {
		match self {
			Self::Owned(v) => v,
			Self::Borrowed(v) => v,
		}
	}
	pub(crate) fn get_mut(&mut self) -> &mut Access {
		match self {
			Self::Owned(v) => v,
			Self::Borrowed(v) => v,
		}
	}
}
pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) authority: Authority<'a>,
	pub(crate) pending: Option<PendingObject>,
}
fn missing() -> Error {
	Error::External("transfer repository scope invariant".into())
}
fn limits(store: &Store) -> Limits {
	let p = &store.capabilities.0;
	Limits {
		admission: p.admission,
		share_files: p.limits.share_files,
		share_file_bytes: p.limits.share_file_bytes,
		share_bytes: p.limits.share_bytes,
		staging_seconds: p.staging_seconds,
		working_bytes: p.working_bytes,
	}
}
impl From<NativeIdentity> for Identity {
	fn from(v: NativeIdentity) -> Self {
		Self {
			transfer_id: v.transfer_id,
			input_digest: v.input_digest,
		}
	}
}
impl From<Description> for NativeDescription {
	fn from(v: Description) -> Self {
		Self {
			protocol: v.protocol,
			transfer_id: v.transfer_id,
			source_node: v.source_node,
			target: v.target.into(),
			source_tenant: v.source_tenant,
			source_subject: v.source_subject,
			source_agent: v.source_agent,
			input_digest: v.input_digest,
			manifest_digest: v.manifest_digest,
			files: v.files.into_iter().map(Into::into).collect(),
			expires_at: v.expires_at,
		}
	}
}
impl From<NativeDescription> for Description {
	fn from(v: NativeDescription) -> Self {
		Self {
			protocol: v.protocol,
			transfer_id: v.transfer_id,
			source_node: v.source_node,
			target: v.target.into(),
			source_tenant: v.source_tenant,
			source_subject: v.source_subject,
			source_agent: v.source_agent,
			input_digest: v.input_digest,
			manifest_digest: v.manifest_digest,
			files: v.files.into_iter().map(Into::into).collect(),
			expires_at: v.expires_at,
		}
	}
}
#[async_trait]
impl TransferRepository for Repository<'_> {
	fn limits(&self) -> Limits {
		limits(&self.federation.store)
	}
	async fn snapshot(&self, id: Uuid) -> Result<Record> {
		let result: NativeResult<NativeRecord> = async {
			let f = self.federation;
			{
				let query_bind_1 = id;
				sqlx::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("transfer_out")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&f.store.pool)
				.await?
			}
			.ok_or_else(|| NativeError::NotFound("transfer unavailable".into()))
		}
		.await;
		result.map(domain).map_err(Into::into)
	}
	async fn begin_sender(&self, record: &Record) -> Result<Box<dyn TransferScope + '_>> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: serde_json::from_value(record.data["credential_id"].clone())?,
			tenant: record.tenant.clone(),
			subject: record.owner.clone(),
		};
		let access = Access::begin(&self.federation.store, &identity).await?;
		Ok(Box::new(Scope {
			store: Some(&self.federation.store),
			authority: Authority::Owned(Box::new(access)),
			pending: None,
		}))
	}
	async fn begin_receipt(&self, id: Uuid) -> Result<Box<dyn ReceiptScope + '_>> {
		let tx = self
			.federation
			.store
			.pool
			.begin()
			.await
			.map_err(NativeError::from)?;
		Ok(Box::new(Receipt { tx, id }))
	}
	async fn request(&self, node: &str, path: &str, body: &Value) -> Result<Value> {
		peer::authority_request(self.federation, node, path, body)
			.await
			.map_err(Into::into)
	}
	async fn jobs(&self) -> Result<Vec<Uuid>> {
		let result: NativeResult<Vec<Uuid>> = async {
			let f = self.federation;

			let ids: Vec<Uuid> = sqlx::query_scalar(
				&Query::select()
					.column(Alias::new("id"))
					.from(Alias::new("core_records"))
					.and_where(
						Expr::col(Alias::new("kind"))
							.eq(reinhardt::query::Expr::value("transfer_out")),
					)
					.and_where(Expr::col(Alias::new("state")).is_in([
						"pending",
						"transferring",
						"committing",
					]))
					.and_where(Expr::cust(
						"COALESCE((data->>'retry_after')::timestamptz, '-infinity'::timestamptz) < CURRENT_TIMESTAMP",
					))
					.limit(8)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&f.store.pool)
			.await?;
			Ok(ids)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn record_failure(&self, id: Uuid, terminal: bool) -> Result<()> {
		let result:NativeResult<()>=async {let mut tx=self.federation.store.pool.begin().await?;
{
					let query_bind_1 = id;
					let query_bind_2 = terminal;
					sqlx::query(&Query::update().table(Alias::new("core_records")).value_expr(Alias::new("state"),SimpleExpr::CustomWithExpr("(CASE WHEN ? THEN 'blocked' WHEN COALESCE((data->>'retry_count')::int,0) >= 11 THEN 'uncertain' ELSE state END)".into(), vec![Expr::value(query_bind_2.to_owned()).into()])).value_expr(Alias::new("data"),Expr::cust("data || jsonb_build_object('error','TRANSFER_PENDING_OR_DENIED','retry_count',COALESCE((data->>'retry_count')::int,0)+1,'retry_after',CURRENT_TIMESTAMP + INTERVAL '5 seconds')")).and_where(Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned()))).and_where(Expr::col(Alias::new("state")).ne(reinhardt::query::Expr::value("delivered"))).to_string(PostgresQueryBuilder)).execute(&mut *tx).await?
				};tx.commit().await?;Ok(())}.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl TransferScope for Scope<'_> {
	fn limits(&self) -> Result<Limits> {
		Ok(limits(self.store.ok_or_else(missing)?))
	}
	fn node_id(&self) -> &str {
		self.store.map_or("", |s| s.node_id.as_str())
	}
	fn principal(&self) -> &str {
		&self.authority.get().identity.subject
	}
	fn tenant(&self) -> &str {
		&self.authority.get().identity.tenant
	}
	fn credential(&self) -> Uuid {
		self.authority.get().identity.credential_id
	}
	fn subjects(&self) -> &[String] {
		&self.authority.get().subjects
	}
	fn set_subjects(&mut self, subjects: Vec<String>) {
		self.authority.get_mut().subjects = subjects;
	}
	fn set_context(&mut self, value: Value) {
		self.authority.get_mut().context = value;
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.authority.get().resource(kind, id, attributes)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.authority
			.get_mut()
			.workspace(id)
			.await
			.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.authority
			.get_mut()
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn authorize(&mut self, area: &Area, action: &str) -> Result<()> {
		sessions::authorize(
			self.authority.get_mut(),
			&NativeArea::from(area.clone()),
			action,
		)
		.await
		.map_err(Into::into)
	}
	async fn sources(&mut self, workspace: Uuid, constraints: &Value) -> Result<()> {
		sessions::authorize_sources(self.authority.get_mut(), workspace, constraints)
			.await
			.map_err(Into::into)
	}
	async fn run(&mut self, id: Uuid) -> Result<RunMetadata> {
		self.authority
			.get_mut()
			.run_for_interaction(id)
			.await
			.map(|r| r.metadata())
			.map_err(Into::into)
	}
	async fn entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(self.authority.get_mut(), reference, action)
			.await
			.map_err(Into::into)
	}
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()> {
		crate::marketplace::check_pinned(self.authority.get_mut(), entry)
			.await
			.map_err(Into::into)
	}
	async fn peer_enabled(&mut self, node: &str) -> Result<Option<bool>> {
		let result: NativeResult<Option<bool>> = async {
			let access = self.authority.get_mut();
			Ok({
				let query_bind_1 = node;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("enabled"))
						.from(Alias::new("peers"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("node_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
						)
						.lock(reinhardt::query::LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn record(&mut self, id: Uuid, kind: &str) -> Result<Record> {
		records::get(self.authority.get_mut(), id, kind)
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn insert(
		&mut self,
		id: Uuid,
		area: Option<Uuid>,
		kind: &str,
		state: &str,
		data: Value,
		expires: Option<DateTime<Utc>>,
	) -> Result<Record> {
		records::insert(
			self.authority.get_mut(),
			id,
			area,
			kind,
			state,
			data,
			expires,
		)
		.await
		.map(domain)
		.map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut row = native(record);
		let result = records::update(self.authority.get_mut(), &mut row).await;
		*record = domain(row);
		result.map_err(Into::into)
	}
	async fn copy_snapshot(&mut self, file: &MountedFile) -> Result<MountedFile> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.copy_object(
				self.authority.get_mut(),
				None,
				"transfer_snapshot",
				&FileEntry::from(file.clone()),
			)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn read_chunk(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>> {
		self.store
			.ok_or_else(missing)?
			.capabilities
			.read_chunk(
				self.authority.get_mut(),
				&FileEntry::from(file.clone()),
				offset,
			)
			.await
			.map_err(Into::into)
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.authority.get_mut(), key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()> {
		sessions::cache(self.authority.get_mut(), key, digest, result)
			.await
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		let Scope { authority, .. } = *self;
		match authority {
			Authority::Owned(access) => (*access)
				.finish(result.map_err(NativeError::from))
				.await
				.map_err(Into::into),
			Authority::Borrowed(_) => Err(missing()),
		}
	}
}
struct Receipt {
	tx: sqlx::Transaction<'static, sqlx::Postgres>,
	id: Uuid,
}
#[async_trait]
impl ReceiptScope for Receipt {
	async fn load(&mut self) -> Result<Record> {
		let result: NativeResult<NativeRecord> = async {
			let id = self.id;
			let tx = &mut self.tx;
			Ok({
				let query_bind_1 = id;
				sqlx::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("transfer_out")),
						)
						.lock(reinhardt::query::LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			})
		}
		.await;
		result.map(domain).map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut row = native(record);
		let result = records::update_committed(&mut self.tx, &mut row).await;
		*record = domain(row);
		result.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		result?;
		self.tx
			.commit()
			.await
			.map_err(NativeError::from)
			.map_err(Into::into)
	}
}
