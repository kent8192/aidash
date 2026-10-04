//! Native lifecycle adapters retain all locks, conditional updates and ownership predicates.
use super::capability_records::{domain, native};
use crate::apps::execution::capabilities::{
	serializers::{
		cleanup::{
			Choice as NativeChoice, Cleanup as NativeCleanup, CleanupResult as NativeCleanupResult,
			ManagedArea as NativeManagedArea, ManagementPage as NativeManagementPage,
			Restore as NativeRestore,
		},
		contracts::{Area as NativeArea, FileEntry},
	},
	services::{
		python,
		records::{self, Record as NativeRecord},
		service, sessions, sharing, thread_lifecycle,
	},
};
use crate::{
	Error as NativeError, Result as NativeResult, authorization::access::Access, store::Store,
};
use aidash_application::{
	Error, Result,
	ports::capabilities::cleanup::{
		CleanupRepository, CleanupScope, Creation, ErasureScope, Limits,
	},
};
use aidash_domain::{
	capabilities::{cleanup::*, operations::MountedFile, records::Record, sessions::Area},
	policy::{PolicyBundle, Resource},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) access: &'a mut Access,
}
pub(crate) struct Repository<'a> {
	pub(crate) store: &'a Store,
}
struct Erasure<'a> {
	store: &'a Store,
	tx: sqlx::Transaction<'static, sqlx::Postgres>,
}
fn missing() -> Error {
	Error::External("cleanup repository scope invariant".into())
}
impl From<NativeCleanup> for Cleanup {
	fn from(v: NativeCleanup) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			expected_revision: v.expected_revision,
			confirmation_id: v.confirmation_id,
			choice: match v.choice {
				NativeChoice::Keep => Choice::Keep,
				NativeChoice::Recoverable => Choice::Recoverable,
				NativeChoice::Irreversible => Choice::Irreversible,
			},
		}
	}
}
impl From<NativeRestore> for Restore {
	fn from(v: NativeRestore) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			expected_revision: v.expected_revision,
			snapshot_id: v.snapshot_id,
			thread_id: v.thread_id,
		}
	}
}
impl From<CleanupResult> for NativeCleanupResult {
	fn from(v: CleanupResult) -> Self {
		Self {
			operation_id: v.operation_id,
			area_id: v.area_id,
			state: v.state,
			revision: v.revision,
			recovery_expires_at: v.recovery_expires_at,
		}
	}
}
impl From<ManagedArea> for NativeManagedArea {
	fn from(v: ManagedArea) -> Self {
		Self {
			area_id: v.area_id,
			workspace_id: v.workspace_id,
			thread_id: v.thread_id,
			agent_id: v.agent_id,
			owner: v.owner,
			state: v.state,
			revision: v.revision,
			generation: v.generation,
			files: v.files,
			bytes: v.bytes,
			snapshot_id: v.snapshot_id,
			recovery_expires_at: v.recovery_expires_at,
			cleanup_operation_id: v.cleanup_operation_id,
		}
	}
}
impl From<ManagementPage> for NativeManagementPage {
	fn from(v: ManagementPage) -> Self {
		Self {
			items: v.items.into_iter().map(Into::into).collect(),
			next_cursor: v.next_cursor,
		}
	}
}
pub(crate) async fn persist(access: &mut Access, area: &NativeArea) -> NativeResult<()> {
	{
		let query_bind_1 = area.id;
		let query_bind_2 = &area.state;
		let query_bind_3 = area.generation;
		let query_bind_4 = area.epoch;
		let query_bind_5 = area.thread_id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_areas"))
				.value_expr(
					Alias::new("state"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("generation"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("epoch"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_4.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("thread_id"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_5.to_owned()).into()],
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?
	};
	Ok(())
}
#[async_trait]
impl CleanupScope for Scope<'_> {
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	fn credential(&self) -> Uuid {
		self.access.identity.credential_id
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	fn limits(&self) -> Result<Limits> {
		let p = &self.store.ok_or_else(missing)?.capabilities.0;
		Ok(Limits {
			working_bytes: p.working_bytes,
			recovery_seconds: p.recovery_seconds,
		})
	}
	fn set_context(&mut self, value: Value) {
		self.access.context = value;
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn serialize(&mut self) -> Result<()> {
		sharing::serialize(self.access).await.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn sources(&mut self, workspace: Uuid, constraints: &Value) -> Result<()> {
		sessions::authorize_sources(self.access, workspace, constraints)
			.await
			.map_err(Into::into)
	}
	async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<()> {
		self.access
			.workspace_record(workspace, "message", id)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn visible_thread(&mut self, thread: Uuid) -> Result<()> {
		thread_lifecycle::visible(&mut self.access.tx, thread)
			.await
			.map_err(Into::into)
	}
	async fn load_area(&mut self, id: Uuid) -> Result<Option<Area>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let area: Option<NativeArea> = {
				let query_bind_1 = &access.identity.tenant;
				let query_bind_2 = id;
				sqlx::query_as(
					&sessions::select("core_areas")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(area)
		}
		.await;
		result.map(|a| a.map(Into::into)).map_err(Into::into)
	}
	async fn inventory(&mut self, cursor: Option<Uuid>) -> Result<Vec<Area>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let rows: Vec<NativeArea> = {
				let query_bind_1 = &access.identity.tenant;
				let query_bind_2 = cursor.unwrap_or(Uuid::nil());
				sqlx::query_as(
					&sessions::select("core_areas")
						.and_where(
							Expr::col(Alias::new("tenant"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_2.to_owned())),
						)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(51)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **access.tx)
				.await?
			};
			Ok(rows)
		}
		.await;
		result
			.map(|r| r.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn inventory_recovery(&mut self, area: &Area) -> Result<Option<Record>> {
		let result:NativeResult<_>=async {let access=&mut *self.access;let row:Option<NativeRecord>={
			let query_bind_1 = area.id;
			let query_bind_2 = area.generation;
			sqlx::query_as(&sessions::select("core_records")
				.and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(query_bind_1.to_owned())))
				.and_where(Expr::col(Alias::new("kind")).eq(reinhardt::query::Expr::value("cleanup")))
				.and_where(SimpleExpr::CustomWithExpr("((state = 'recoverable' OR (state = 'kept' AND data->'retained' = 'true'::jsonb)) AND (data->>'generation')::bigint = ?)".into(), vec![Expr::value(query_bind_2.to_owned()).into()]))
                .order_by(Alias::new("expires_at"), Order::Desc)
				.limit(1)
				.to_string(PostgresQueryBuilder))
        .fetch_optional(&mut **access.tx)
		.await?
		};Ok(row)}.await;
		result.map(|r| r.map(domain)).map_err(Into::into)
	}
	async fn cleanup_operation(&mut self, area: &Area) -> Result<Option<Uuid>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let id = {
				let query_bind_1 = area.id;
				let query_bind_2 = area.generation;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("core_records"))
						.and_where(
							Expr::col(Alias::new("area_id"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("cleanup")),
						)
						.and_where(
							Expr::col(Alias::new("state")).is_in(["deleting", "cleanup_failed"]),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"((data->>'generation')::bigint = ?)".into(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.order_by(Alias::new("id"), Order::Asc)
						.limit(1)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(id)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn recoverable(&mut self, area: &Area) -> Result<Option<Record>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let record: Option<NativeRecord> = {
				let query_bind_1 = area.id;
				let query_bind_2 = area.generation;
				sqlx::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("area_id"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("cleanup")),
						)
						.and_where(
							Expr::col(Alias::new("state"))
								.eq(reinhardt::query::Expr::value("recoverable")),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"((data->>'generation')::bigint = ?)".into(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(record)
		}
		.await;
		result.map(|r| r.map(domain)).map_err(Into::into)
	}
	async fn record_area(&mut self, id: Uuid) -> Result<Option<Option<Uuid>>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let area = {
				let query_bind_1 = id;
				let query_bind_2 = &access.identity.tenant;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("area_id"))
						.from(Alias::new("core_records"))
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("tenant"))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("cleanup")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(area)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn thread_root(&mut self, area: &Area, thread: Uuid) -> Result<Option<Uuid>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let root = {
				let query_bind_1 = thread;
				let query_bind_2 = area.workspace_id;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("root_message_id"))
						.from(Alias::new("channel_threads"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"workspace_id",
							)))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(root)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn occupied(&mut self, area: &Area, thread: Uuid) -> Result<Option<Uuid>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let id = {
				let query_bind_1 = &area.tenant;
				let query_bind_2 = &area.home_node;
				let query_bind_3 = area.workspace_id;
				let query_bind_4 = thread;
				let query_bind_5 = &area.agent_id;
				let query_bind_6 = &area.owner;
				let query_bind_7 = area.id;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("core_areas"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("home_node")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
								"workspace_id",
							)))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("agent_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("owner"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).ne(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_7.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(id)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn reattach_working(&mut self, area: &Area, file: Uuid) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = file;
				let query_bind_2 = area.id;
				let query_bind_3 = &area.tenant;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_objects"))
						.value(Alias::new("kind"), "working")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("area_id"))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("tenant"))
								.eq(Expr::value(query_bind_3.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("recovery")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn cancel_runs(&mut self, area: &Area) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = area.id;
				sqlx::query(
					&Query::update()
						.table(Alias::new("runs"))
						.value(Alias::new("control"), "CANCELLED")
						.and_where(
							Expr::col(Alias::new("id")).in_subquery(
								Query::select()
									.column(Alias::new("run_id"))
									.from(Alias::new("core_runs"))
									.and_where(
										Expr::col(Alias::new("area_id"))
											.eq(Expr::value(query_bind_1.to_owned())),
									)
									.to_owned(),
							),
						)
						.and_where(Expr::col(Alias::new("phase")).is_not_in([
							"COMPLETED",
							"FAILED",
							"CANCELLED",
						]))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn revoke_grants(&mut self, area: &Area) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = area.id;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_records"))
						.value(Alias::new("state"), "revoked")
						.and_where(
							Expr::col(Alias::new("area_id"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(Expr::col(Alias::new("kind")).is_in(["grant", "outbound"]))
						.and_where(Expr::col(Alias::new("state")).is_in([
							"active",
							"approved",
							"pending",
							"executing",
						]))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn load_record(&mut self, id: Uuid, kind: &str) -> Result<Record> {
		records::get(self.access, id, kind)
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn create(&mut self, r: Creation) -> Result<Record> {
		records::insert(
			self.access,
			r.id,
			r.area,
			r.kind,
			r.state,
			r.data,
			r.expires,
		)
		.await
		.map(domain)
		.map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut row = native(record);
		records::update(self.access, &mut row).await?;
		*record = domain(row);
		Ok(())
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.access, key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()> {
		sessions::cache(self.access, key, digest, result)
			.await
			.map_err(Into::into)
	}
	async fn release_python(&mut self, area: &Area, reason: &str) -> Result<()> {
		python::release(
			self.store.ok_or_else(missing)?,
			self.access,
			&area.clone().into(),
			reason,
		)
		.await
		.map_err(Into::into)
	}
	async fn copy_owned(
		&mut self,
		area: Uuid,
		kind: &str,
		file: &MountedFile,
	) -> Result<MountedFile> {
		let row: FileEntry = file.clone().into();
		self.store
			.ok_or_else(missing)?
			.capabilities
			.copy_owned(self.access, area, kind, &row)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn verified(&mut self, file: &MountedFile) -> Result<()> {
		let row: FileEntry = file.clone().into();
		self.store
			.ok_or_else(missing)?
			.capabilities
			.verified(self.access, &row)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn publish(&mut self, area: &mut Area) -> Result<()> {
		let mut row: NativeArea = area.clone().into();
		let result = service::publish(self.store.ok_or_else(missing)?, self.access, &mut row).await;
		*area = row.into();
		result.map_err(Into::into)
	}
	async fn persist(&mut self, area: &Area, restoration: bool) -> Result<()> {
		let result = persist(self.access, &area.clone().into()).await;
		result.map_err(|error| match error {
			NativeError::Database(ref db)
				if restoration
					&& db.as_database_error().is_some_and(|error| {
						error.is_unique_violation()
							&& error.constraint() == Some("core_session_identity")
					}) =>
			{
				Error::Conflict("RESTORATION_THREAD_OCCUPIED".into())
			}
			error => error.into(),
		})
	}
}
#[async_trait]
impl CleanupRepository for Repository<'_> {
	async fn jobs(&self, after: Uuid) -> Result<Vec<Record>> {
		let store = self.store;
		let result: NativeResult<Vec<NativeRecord>> = async {
			let jobs = {
				let query_bind_1 = after;
				sqlx::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("cleanup")),
						)
						.and_where(
							Condition::any()
								.add(
									Expr::col(Alias::new("state"))
										.is_in(["deleting", "cleanup_failed"]),
								)
								.add(
									Condition::all()
										.add(
											Expr::col(Alias::new("state"))
												.is_in(["restored", "recoverable"]),
										)
										.add(
											reinhardt::query::SimpleExpr::from(Expr::col(
												Alias::new("expires_at"),
											))
											.lte(Expr::cust("CURRENT_TIMESTAMP")),
										),
								),
						)
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())),
						)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(8)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&store.pool)
				.await?
			};
			Ok(jobs)
		}
		.await;
		result
			.map(|rows| rows.into_iter().map(domain).collect())
			.map_err(Into::into)
	}
	async fn begin(&self) -> Result<Box<dyn ErasureScope + '_>> {
		Ok(Box::new(Erasure {
			store: self.store,
			tx: self.store.pool.begin().await.map_err(NativeError::from)?,
		}))
	}
	async fn failure(&self, id: Uuid, area: Option<Uuid>) -> Result<()> {
		let result: NativeResult<()> = async {
			let mut tx = self.store.pool.begin().await?;
			{
				let query_bind_1 = area;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_areas"))
						.value(Alias::new("state"), "cleanup_failed")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("state"))
								.eq(reinhardt::query::Expr::value("cleaning")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			{
				let query_bind_1 = id;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_records"))
						.value(Alias::new("state"), "cleanup_failed")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("state"))
								.eq(reinhardt::query::Expr::value("deleting")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			tx.commit().await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl ErasureScope for Erasure<'_> {
	async fn locked(&mut self, snapshot: &Record) -> Result<(Area, Record)> {
		let result: NativeResult<(NativeArea, NativeRecord)> = async {
			let tx = &mut self.tx;
			let area: NativeArea = {
				let query_bind_1 = snapshot.area_id.ok_or(Error::Forbidden)?;
				let query_bind_2 = &snapshot.tenant;
				sqlx::query_as(
					&sessions::select("core_areas")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			};
			let record: NativeRecord = {
				let query_bind_1 = snapshot.id;
				let query_bind_2 = &area.tenant;
				sqlx::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("cleanup")),
						)
						.and_where(
							Expr::col(Alias::new("tenant"))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			};
			Ok((area, record))
		}
		.await;
		result
			.map(|(a, r)| (a.into(), domain(r)))
			.map_err(Into::into)
	}
	async fn objects(&mut self, area: &Area) -> Result<Vec<Uuid>> {
		let result: NativeResult<_> = async {
			let tx = &mut self.tx;
			let ids = {
				let query_bind_1 = &area.tenant;
				let query_bind_2 = area.id;
				sqlx::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("core_objects"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("area_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(&mut **tx)
				.await?
			};
			Ok(ids)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn erase(&mut self, tenant: &str, id: Uuid) -> Result<()> {
		self.store
			.capabilities
			.erase_committed(&mut self.tx, tenant, id)
			.await
			.map_err(Into::into)
	}
	async fn update_area(&mut self, area: &Area) -> Result<()> {
		let result: NativeResult<()> = async {
			let tx = &mut self.tx;
			{
				let query_bind_1 = area.id;
				let query_bind_2 = &area.state;
				let query_bind_3 = area.generation;
				let query_bind_4 = area.epoch;
				let query_bind_5 = area.revision;
				sqlx::query(
					&Query::update()
						.table(Alias::new("core_areas"))
						.value_expr(
							Alias::new("state"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						)
						.value_expr(
							Alias::new("generation"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							),
						)
						.value_expr(
							Alias::new("epoch"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()],
							),
						)
						.value_expr(
							Alias::new("revision"),
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()],
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **tx)
				.await?
			};
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn update_record(&mut self, record: &mut Record) -> Result<()> {
		let mut row = native(record);
		records::update_committed(&mut self.tx, &mut row).await?;
		*record = domain(row);
		Ok(())
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.event(&mut self.tx, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()> {
		match result {
			Ok(true) => self
				.tx
				.commit()
				.await
				.map_err(NativeError::from)
				.map_err(Into::into),
			Ok(false) => Ok(()),
			Err(error) => Err(error),
		}
	}
}
