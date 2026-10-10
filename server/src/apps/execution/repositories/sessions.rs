//! Session queries retain row locks, queue order, advisory locks and one caller transaction.
use crate::apps::execution::capabilities::{
	serializers::contracts::{Area as NativeArea, SessionRun as NativeSessionRun},
	services::{references, skills, thread_lifecycle},
};
use crate::{
	Error as NativeError, Result as NativeResult, authorization::access::Access, domain::Task,
	store::Store,
};
use aidash_application::{Error, Result, ports::capabilities::sessions::SessionScope};
use aidash_domain::{
	capabilities::sessions::{Area, SessionRun},
	policy::Resource,
	registry::{AgentConfig, EntityRef},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait as _, LockType, OnConflict, Order, PostgresQueryBuilder,
	Query, QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) access: &'a mut Access,
}
impl Scope<'_> {
	fn store(&self) -> Result<&Store> {
		self.store
			.ok_or_else(|| Error::External("session repository scope invariant".into()))
	}
}
pub(crate) fn select(table: &str) -> reinhardt::query::SelectStatement {
	Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new(table))
		.to_owned()
}
#[async_trait]
impl SessionScope for Scope<'_> {
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn admission(&self) -> Result<bool> {
		Ok(self.store()?.capabilities.0.admission)
	}
	fn set_context(&mut self, context: Value) {
		self.access.context = context;
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
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
	async fn message(&mut self, workspace: Uuid, id: Uuid) -> Result<()> {
		self.access
			.workspace_record(workspace, "message", id)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn agent(&mut self, reference: &EntityRef) -> Result<()> {
		crate::authorization::catalog::entry(self.access, reference, "agent.execute")
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn reference(&mut self, id: Uuid) -> Result<()> {
		references::get(self.access, id, "reference.read")
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn bind_task(&mut self, task: Uuid, thread: Uuid) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = task;
				let query_bind_2 = thread;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("core_task_sessions"))
						.columns(["task_id", "thread_id"].map(Alias::new))
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.to_owned(),
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
	async fn load(&mut self, id: Uuid) -> Result<Option<Area>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			// Batched reads share the row so they run concurrently; writers still exclude them.
			let lock = if access.shared_area {
				LockType::Share
			} else {
				LockType::Update
			};
			let area: Option<NativeArea> = {
				let query_bind_1 = id;
				let query_bind_2 = &access.identity.tenant;
				crate::database::native::query_as(
					&select("core_areas")
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
						.lock(lock)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(area)
		}
		.await;
		result
			.map(|value| value.map(Into::into))
			.map_err(Into::into)
	}
	async fn run_binding(&mut self, run_id: Uuid) -> Result<Option<(Uuid, i64)>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let row = {
				let query_bind_1 = run_id;
				crate::database::native::query_as(
					&Query::select()
						.columns(["area_id", "generation"].map(Alias::new))
						.from(Alias::new("core_runs"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["area_id", "generation"])
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(row)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn current_run(&mut self, area: &Area) -> Result<Option<Uuid>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let current = {
				let query_bind_1 = area.id;
				let query_bind_2 = area.generation;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("run_id"))
						.from(Alias::new("core_runs"))
						.and_where(
							Expr::col(Alias::new("area_id"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("generation"))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("initialized"))
								.eq(reinhardt::query::Expr::value(true)),
						)
						.order_by(Alias::new("sequence"), Order::Desc)
						.limit(1)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **access.tx)
				.await?
			};
			Ok(current)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn context_area(&mut self, run_id: Uuid) -> Result<Option<Area>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let area: Option<NativeArea> = {
				let query_bind_1 = run_id;
				crate::database::native::query_as(
					&select("core_areas")
						.and_where(
							Expr::col(Alias::new("id")).in_subquery(
								Query::select()
									.column(Alias::new("area_id"))
									.from(Alias::new("core_runs"))
									.and_where(
										reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
											"run_id",
										)))
										.eq(SimpleExpr::CustomWithExpr(
											"(?)".to_owned(),
											vec![Expr::value(query_bind_1.to_owned()).into()],
										)),
									)
									.and_where(Expr::col(Alias::new("generation")).equals((
										Alias::new("core_areas"),
										Alias::new("generation"),
									)))
									.to_owned(),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["area_id"])
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(area)
		}
		.await;
		result
			.map(|value| value.map(Into::into))
			.map_err(Into::into)
	}
	async fn explicit_thread(&mut self, task_id: Uuid) -> Result<Option<Uuid>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let row = {
				let query_bind_1 = task_id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("thread_id"))
						.from(Alias::new("core_task_sessions"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("task_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								)),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **access.tx)
				.await?
			};
			Ok(row)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let task: Task = {
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&select("tasks")
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
				.fetch_one(&mut **access.tx)
				.await?
			};
			Ok(task)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn parent_areas(&mut self, id: Uuid) -> Result<Vec<Area>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let areas: Vec<NativeArea> = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&select("core_areas")
						.and_where(
							Expr::col(Alias::new("id")).in_subquery(
								Query::select()
									.column(Alias::new("area_id"))
									.from(Alias::new("core_runs"))
									.and_where(
										Expr::col(Alias::new("run_id")).in_subquery(
											Query::select()
												.column(Alias::new("id"))
												.from(Alias::new("runs"))
												.and_where(
													Expr::col(Alias::new("task_id"))
														.eq(Expr::value(query_bind_1.to_owned())),
												)
												.and_where(
													Expr::col(Alias::new("phase")).is_not_in([
														"COMPLETED",
														"FAILED",
														"CANCELLED",
													]),
												)
												.to_owned(),
										),
									)
									.to_owned(),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["area_id"])
				.fetch_all(&mut **access.tx)
				.await?
			};
			Ok(areas)
		}
		.await;
		result
			.map(|rows| rows.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn lock_thread(&mut self, thread: Uuid, workspace: Uuid) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let _channel: Option<Uuid> = {
				let query_bind_1 = thread;
				let query_bind_2 = workspace;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
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
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **access.tx)
				.await?
			};
			thread_lifecycle::visible(&mut access.tx, thread).await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn ensure_area(&mut self, task: &Task, thread: Uuid, agent_id: &str) -> Result<Area> {
		let result: NativeResult<_> = async {
			let store = self
				.store
				.ok_or_else(|| NativeError::Invalid("session repository scope invariant".into()))?;
			let access = &mut *self.access;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("core_areas"))
					.columns(
						[
							"id",
							"tenant",
							"home_node",
							"workspace_id",
							"thread_id",
							"agent_id",
							"owner",
						]
						.map(Alias::new),
					)
					.from_subquery(((1..=7).map(|i| Expr::cust(format!("${i}")))).fold(
						reinhardt::query::Query::select(),
						|mut select, expr| {
							select.expr(expr);
							select
						},
					))
					.on_conflict(
						OnConflict::columns(
							[
								"tenant",
								"home_node",
								"workspace_id",
								"thread_id",
								"agent_id",
								"owner",
							]
							.map(Alias::new),
						)
						.do_nothing()
						.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.bind(Uuid::new_v4())
			.bind(&access.identity.tenant)
			.bind(&store.node_id)
			.bind(task.workspace_id)
			.bind(thread)
			.bind(agent_id)
			.bind(&access.identity.subject)
			.execute(&mut **access.tx)
			.await?;
			let area: NativeArea = {
				let query_bind_1 = &access.identity.tenant;
				let query_bind_2 = task.workspace_id;
				let query_bind_3 = thread;
				let query_bind_4 = agent_id;
				let query_bind_5 = &store.node_id;
				let query_bind_6 = &access.identity.subject;
				crate::database::native::query_as(
					&select("core_areas")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
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
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("agent_id")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("home_node")))
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
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **access.tx)
				.await?
			};
			Ok(area)
		}
		.await;
		result.map(Into::into).map_err(Into::into)
	}
	async fn initialized(&mut self, run_id: Uuid) -> Result<Option<bool>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let initialized = {
				let query_bind_1 = run_id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("initialized"))
						.from(Alias::new("core_runs"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **access.tx)
				.await?
			};
			Ok(initialized)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn initialized_locked(&mut self, run_id: Uuid) -> Result<bool> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let initialized = {
				let query_bind_1 = run_id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("initialized"))
						.from(Alias::new("core_runs"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **access.tx)
				.await?
			};
			Ok(initialized)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn mark_initialized(&mut self, run_id: Uuid) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = run_id;
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("core_runs"))
						.value(Alias::new("initialized"), true)
						.and_where(
							Expr::col(Alias::new("run_id"))
								.eq(Expr::value(query_bind_1.to_owned())),
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
	async fn pin(&mut self, run_id: Uuid, area: &mut Area, config: &AgentConfig) -> Result<()> {
		let store = self
			.store
			.ok_or_else(|| Error::External("session repository scope invariant".into()))?;
		let mut native: NativeArea = area.clone().into();
		let result: NativeResult<()> = async {
			references::pin(store, self.access, &mut native, config).await?;
			skills::pin(store, self.access, run_id, &native, config).await
		}
		.await;
		*area = native.into();
		result.map_err(Into::into)
	}
	async fn enqueue(&mut self, run_id: Uuid, area: &Area, initialized: bool) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			{
				let query_bind_1 = run_id;
				let query_bind_2 = area.id;
				let query_bind_3 = area.next_sequence;
				let query_bind_4 = area.generation;
				let query_bind_5 = initialized;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("core_runs"))
						.columns(
							["run_id", "area_id", "sequence", "generation", "initialized"]
								.map(Alias::new),
						)
						.from_subquery(
							reinhardt::query::Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								))
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			{
				let query_bind_1 = area.id;
				crate::database::native::query(
					&Query::update()
						.table(Alias::new("core_areas"))
						.value_expr(
							Alias::new("next_sequence"),
							Expr::col(Alias::new("next_sequence"))
								.add(reinhardt::query::Expr::value(1)),
						)
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
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
	async fn queue(&mut self, area: &Area) -> Result<Vec<SessionRun>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let rows: Vec<NativeSessionRun> = {
				let query_bind_1 = area.id;
				let query_bind_2 = area.generation;
				crate::database::native::query_as(
					&Query::select()
						.column((Alias::new("q"), Alias::new("run_id")))
						.column((Alias::new("q"), Alias::new("sequence")))
						.column((Alias::new("r"), Alias::new("phase")))
						.column((Alias::new("r"), Alias::new("control")))
						.from_as(Alias::new("core_runs"), Alias::new("q"))
						.join(
							reinhardt::query::JoinType::InnerJoin,
							reinhardt::query::TableRef::table_alias(
								Alias::new("runs"),
								Alias::new("r"),
							),
							Expr::col((Alias::new("q"), Alias::new("run_id")))
								.equals((Alias::new("r"), Alias::new("id"))),
						)
						.and_where(
							Expr::col((Alias::new("q"), Alias::new("area_id")))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col((Alias::new("q"), Alias::new("generation")))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.and_where(
							Expr::col((Alias::new("r"), Alias::new("phase"))).is_not_in([
								"COMPLETED",
								"FAILED",
								"CANCELLED",
							]),
						)
						.order_by((Alias::new("q"), Alias::new("sequence")), Order::Asc)
						.limit(101)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["run_id", "sequence", "phase", "control"])
				.fetch_all(&mut **access.tx)
				.await?
			};
			Ok(rows)
		}
		.await;
		result
			.map(|rows| rows.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn last(&mut self, area: &Area) -> Result<Option<(Uuid, String)>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let last = {
				let query_bind_1 = area.id;
				let query_bind_2 = area.generation;
				crate::database::native::query_as(
					&Query::select()
						.column((Alias::new("q"), Alias::new("run_id")))
						.column((Alias::new("r"), Alias::new("agent_version")))
						.from_as(Alias::new("core_runs"), Alias::new("q"))
						.join(
							reinhardt::query::JoinType::InnerJoin,
							reinhardt::query::TableRef::table_alias(
								Alias::new("runs"),
								Alias::new("r"),
							),
							Expr::col((Alias::new("q"), Alias::new("run_id")))
								.equals((Alias::new("r"), Alias::new("id"))),
						)
						.and_where(
							Expr::col((Alias::new("q"), Alias::new("area_id")))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col((Alias::new("q"), Alias::new("generation")))
								.eq(Expr::value(query_bind_2.to_owned())),
						)
						.order_by((Alias::new("q"), Alias::new("sequence")), Order::Desc)
						.limit(1)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["run_id", "agent_version"])
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(last)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn previous(&mut self, key: Uuid) -> Result<Option<(String, Value)>> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			let lock = format!(
				"core:{}:{}:{key}",
				access.identity.tenant, access.identity.subject
			);
			{
				let query_bind_1 = lock;
				crate::database::native::query(
					&Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(pg_advisory_xact_lock(hashtextextended(?, 0)))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **access.tx)
				.await?
			};
			let previous = {
				let query_bind_1 = &access.identity.tenant;
				let query_bind_2 = &access.identity.subject;
				let query_bind_3 = key;
				crate::database::native::query_as(
					&Query::select()
						.columns(["digest", "result"].map(Alias::new))
						.from(Alias::new("core_requests"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("principal")))
								.eq(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								)),
						)
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("key"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["digest", "result"])
				.fetch_optional(&mut **access.tx)
				.await?
			};
			Ok(previous)
		}
		.await;
		result.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, result: &Value) -> Result<()> {
		let result: NativeResult<_> = async {
			let access = &mut *self.access;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("core_requests"))
					.columns(["tenant", "principal", "key", "digest", "result"].map(Alias::new))
					.from_subquery(((1..=5).map(|i| Expr::cust(format!("${i}")))).fold(
						reinhardt::query::Query::select(),
						|mut select, expr| {
							select.expr(expr);
							select
						},
					))
					.to_string(PostgresQueryBuilder),
			)
			.bind(&access.identity.tenant)
			.bind(&access.identity.subject)
			.bind(key)
			.bind(digest)
			.bind(result)
			.execute(&mut **access.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.ok_or_else(|| Error::External("session repository scope invariant".into()))?
			.event(&mut self.access.tx, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
