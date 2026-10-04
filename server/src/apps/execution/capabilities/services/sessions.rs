use super::contracts::{Area, SessionRun, SessionStatus};
use crate::{
	Error, Result,
	authorization::access::Access,
	domain::{Run, Task},
	registry::AgentConfig,
	store::Store,
};
use reinhardt::query::{
	Alias, ColumnRef, Expr, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) fn select(table: &str) -> reinhardt::query::SelectStatement {
	Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new(table))
		.to_owned()
}
pub(crate) async fn bind_task(access: &mut Access, task: Uuid, thread: Uuid) -> Result<()> {
	{
		let query_bind_1 = task;
		let query_bind_2 = thread;
		sqlx::query(
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
pub(crate) async fn load(access: &mut Access, id: Uuid) -> Result<Area> {
	let area: Area = {
		let query_bind_1 = id;
		let query_bind_2 = &access.identity.tenant;
		sqlx::query_as(
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
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	authorize(access, &area, "file.read").await?;
	Ok(area)
}
pub(crate) async fn authorize(access: &mut Access, area: &Area, action: &str) -> Result<()> {
	let workspace = access.workspace(area.workspace_id).await?;
	access.context = workspace.attributes.clone();
	if area.tenant != access.identity.tenant || area.owner != access.identity.subject {
		return Err(Error::NotFound("working area unavailable".into()));
	}
	let resource = access.resource(
		"working_area",
		area.id,
		json!({"owner":area.owner,"agent_id":area.agent_id,"thread_id":area.thread_id}),
	);
	access.require(&workspace, "workspace.read").await?;
	access
		.require(&resource, action)
		.await
		.map_err(|_| Error::NotFound("working area unavailable".into()))?;
	authorize_sources(access, area.workspace_id, &area.constraints).await
}
pub(crate) async fn authorize_sources(
	access: &mut Access,
	workspace: Uuid,
	constraints: &Value,
) -> Result<()> {
	// Every contributing source remains a required disclosure constraint, including outputs derived from it.
	for source in constraints
		.as_array()
		.ok_or_else(|| Error::Conflict("invalid area constraints".into()))?
	{
		match source["kind"].as_str() {
			Some("message") => {
				let id = serde_json::from_value(source["id"].clone())?;
				access.workspace_record(workspace, "message", id).await?;
			}
			Some("reference_text") => {
				let reference = serde_json::from_value(source["agent"].clone())?;
				crate::authorization::catalog::entry(access, &reference, "agent.execute").await?;
			}
			Some("reference") => {
				super::references::get(
					access,
					serde_json::from_value(source["id"].clone())?,
					"reference.read",
				)
				.await?;
			}
			Some("received_scope") => {
				let owner = source["owner"].as_str().ok_or(Error::Forbidden)?;
				let agent = source["agent"].as_str().ok_or(Error::Forbidden)?;
				if !received_scope_matches(&access.subjects, owner, agent) {
					return Err(Error::Forbidden);
				}
			}
			_ => return Err(Error::Forbidden),
		}
	}
	Ok(())
}
fn received_scope_matches(subjects: &[String], owner: &str, agent: &str) -> bool {
	subjects.first().map(String::as_str) == Some(owner)
		&& subjects.last().map(String::as_str) == Some(agent)
}
pub(crate) async fn for_run(access: &mut Access, run: &Run) -> Result<Area> {
	let (id, generation): (Uuid, i64) = {
		let query_bind_1 = run.id;
		sqlx::query_as(
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
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	let area = load(access, id).await?;
	if area.agent_id != run.agent_id
		|| area.generation != generation
		|| area.workspace_id != run.workspace_id
		|| area.home_node != run.home_node
	{
		return Err(Error::Forbidden);
	}
	Ok(area)
}
/// File tools act on the current session only. A completed predecessor still
/// maps to the same Area row, whose manifest may already belong to a successor.
pub(crate) async fn require_current_run(access: &mut Access, area: &Area, run: &Run) -> Result<()> {
	let current: Option<Uuid> = {
		let query_bind_1 = area.id;
		let query_bind_2 = area.generation;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("run_id"))
				.from(Alias::new("core_runs"))
				.and_where(
					Expr::col(Alias::new("area_id")).eq(Expr::value(query_bind_1.to_owned())),
				)
				.and_where(
					Expr::col(Alias::new("generation")).eq(Expr::value(query_bind_2.to_owned())),
				)
				.and_where(
					Expr::col(Alias::new("initialized")).eq(reinhardt::query::Expr::value(true)),
				)
				.order_by(Alias::new("sequence"), Order::Desc)
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	};
	if current == Some(run.id) {
		Ok(())
	} else {
		Err(Error::NotFound("working area unavailable".into()))
	}
}
/// Revalidate retained context without taking a mutation lock held across a tool's transaction.
pub(crate) async fn context_authority(
	access: &mut Access,
	run: impl Into<crate::domain::RunMetadata>,
) -> Result<()> {
	let run = run.into();
	let area: Area = {
		let query_bind_1 = run.id;
		sqlx::query_as(
			&select("core_areas")
				.and_where(
					Expr::col(Alias::new("id")).in_subquery(
						Query::select()
							.column(Alias::new("area_id"))
							.from(Alias::new("core_runs"))
							.and_where(
								reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("run_id")))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
							)
							.and_where(
								Expr::col(Alias::new("generation"))
									.equals((Alias::new("core_areas"), Alias::new("generation"))),
							)
							.to_owned(),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	authorize(access, &area, "file.read").await?;
	if matches!(
		area.state.as_str(),
		"deleted" | "recoverable" | "uncertain" | "cleaning" | "cleanup_failed" | "retained"
	) {
		return Err(Error::Conflict(
			"AREA_UNAVAILABLE: retained context cannot be reused".into(),
		));
	}
	Ok(())
}
pub(crate) async fn prepare_admission(
	store: &Store,
	access: &mut Access,
	task: &Task,
	config: &AgentConfig,
	agent_id: &str,
) -> Result<Option<Uuid>> {
	if !config.core_capabilities.enabled() {
		return Ok(None);
	}
	if !store.capabilities.0.admission {
		return Err(Error::Conflict("CAPABILITIES_DISABLED: enable the operator execution profile before admitting core work".into()));
	}
	let explicit: Option<Uuid> = {
		let query_bind_1 = task.id;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("thread_id"))
				.from(Alias::new("core_task_sessions"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("task_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	};
	let mut thread = explicit.unwrap_or(task.id);
	let mut ancestor = task.parent_id;
	for _ in 0..64 {
		let Some(id) = ancestor else {
			break;
		};
		let parent: Task = {
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
		let parent_areas: Vec<Area> = {
			let query_bind_1 = id;
			sqlx::query_as(
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
											.and_where(Expr::col(Alias::new("phase")).is_not_in([
												"COMPLETED",
												"FAILED",
												"CANCELLED",
											]))
											.to_owned(),
									),
								)
								.to_owned(),
						),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_all(&mut **access.tx)
			.await?
		};
		for parent_area in parent_areas {
			if parent_area.agent_id == agent_id && parent_area.owner == access.identity.subject {
				return Err(Error::Conflict(
					"SESSION_DEPENDENCY_CYCLE: child would wait behind its parent".into(),
				));
			}
			if explicit.is_none() {
				thread = parent_area.thread_id;
			}
		}
		ancestor = parent.parent_id;
	}
	if ancestor.is_some() {
		return Err(Error::Conflict(
			"SESSION_DEPENDENCY_CYCLE: ancestor limit exceeded".into(),
		));
	}
	// Serialize inherited as well as explicit sessions with conversation deletion.
	// Standalone tasks have no channel_threads row, but still check tombstones.
	let _channel: Option<Uuid> = {
		let query_bind_1 = thread;
		let query_bind_2 = task.workspace_id;
		sqlx::query_scalar(
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
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id"))).eq(
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
	super::thread_lifecycle::visible(&mut access.tx, thread).await?;
	Ok(Some(thread))
}
pub(crate) async fn admit(
	store: &Store,
	access: &mut Access,
	task: &Task,
	run_id: Uuid,
	thread: Option<Uuid>,
	config: &AgentConfig,
	agent_id: &str,
) -> Result<()> {
	let Some(thread) = thread else {
		return Ok(());
	};
	sqlx::query(
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
	let mut area: Area = {
		let query_bind_1 = &access.identity.tenant;
		let query_bind_2 = task.workspace_id;
		let query_bind_3 = thread;
		let query_bind_4 = agent_id;
		let query_bind_5 = &store.node_id;
		let query_bind_6 = &access.identity.subject;
		sqlx::query_as(
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
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("agent_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("home_node"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_5.to_owned()).into()],
						),
					),
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
	authorize(access, &area, "file.read").await?;
	if !matches!(area.state.as_str(), "active" | "running") {
		return Err(Error::Conflict(
			"AREA_UNAVAILABLE: restore or recreate the area before starting work".into(),
		));
	}
	// Queuing cannot mutate mounts or capture discovered Skills while an earlier
	// Run owns the session. Pin them exactly once when this Run becomes active.
	let queue = status(access, &area).await?.queue;
	if queue.len() >= 100 {
		return Err(Error::Conflict("QUEUE_LIMIT".into()));
	}
	let initialized = area.state == "active" && queue.is_empty();
	if initialized {
		super::references::pin(store, access, &mut area, config).await?;
		super::skills::pin(store, access, run_id, &area, config).await?;
	}
	{
		let query_bind_1 = run_id;
		let query_bind_2 = area.id;
		let query_bind_3 = area.next_sequence;
		let query_bind_4 = area.generation;
		let query_bind_5 = initialized;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_runs"))
				.columns(
					["run_id", "area_id", "sequence", "generation", "initialized"].map(Alias::new),
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
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_areas"))
				.value_expr(
					Alias::new("next_sequence"),
					Expr::col(Alias::new("next_sequence")).add(reinhardt::query::Expr::value(1)),
				)
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?
	};
	store
		.event(
			&mut access.tx,
			Some(task.workspace_id),
			"capability.queued",
			json!({"area_id":area.id,"run_id":run_id,"sequence":area.next_sequence}),
		)
		.await?;
	Ok(())
}
pub(crate) async fn initialize(
	store: &Store,
	access: &mut Access,
	run: &Run,
	config: &AgentConfig,
) -> Result<()> {
	let initialized: bool = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
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
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or_else(|| Error::NotFound("working area unavailable".into()))?;
	if initialized {
		return Ok(());
	}
	let mut area = for_run(access, run).await?;
	if status(access, &area).await?.active_run_id != Some(run.id) {
		return Err(Error::Conflict(
			"RUN_QUEUED: wait for the preceding Run".into(),
		));
	}
	super::service::available(&area)?;
	// Admission and initialization both hold the area lock. Recheck after waiting
	// for it, so simultaneous API and worker requests cannot pin twice.
	let initialized: bool = {
		let query_bind_1 = run.id;
		sqlx::query_scalar(
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
		.fetch_one(&mut **access.tx)
		.await?
	};
	if initialized {
		return Ok(());
	}
	super::references::pin(store, access, &mut area, config).await?;
	super::skills::pin(store, access, run.id, &area, config).await?;
	{
		let query_bind_1 = run.id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_runs"))
				.value(Alias::new("initialized"), true)
				.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(query_bind_1.to_owned())))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?
	};
	Ok(())
}
pub(crate) async fn status(access: &mut Access, area: &Area) -> Result<SessionStatus> {
	let rows: Vec<SessionRun> = {
		let query_bind_1 = area.id;
		let query_bind_2 = area.generation;
		sqlx::query_as(
			&Query::select()
				.column((Alias::new("q"), Alias::new("run_id")))
				.column((Alias::new("q"), Alias::new("sequence")))
				.column((Alias::new("r"), Alias::new("phase")))
				.column((Alias::new("r"), Alias::new("control")))
				.from_as(Alias::new("core_runs"), Alias::new("q"))
				.join(
					reinhardt::query::JoinType::InnerJoin,
					reinhardt::query::TableRef::table_alias(Alias::new("runs"), Alias::new("r")),
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
		.fetch_all(&mut **access.tx)
		.await?
	};
	if rows.len() > 100 {
		return Err(Error::Conflict("QUEUE_LIMIT".into()));
	}
	let last: Option<(Uuid, String)> = {
		let query_bind_1 = area.id;
		let query_bind_2 = area.generation;
		sqlx::query_as(
			&Query::select()
				.column((Alias::new("q"), Alias::new("run_id")))
				.column((Alias::new("r"), Alias::new("agent_version")))
				.from_as(Alias::new("core_runs"), Alias::new("q"))
				.join(
					reinhardt::query::JoinType::InnerJoin,
					reinhardt::query::TableRef::table_alias(Alias::new("runs"), Alias::new("r")),
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
		.fetch_optional(&mut **access.tx)
		.await?
	};
	Ok(SessionStatus {
		last_run_id: last.as_ref().map(|r| r.0),
		last_agent_version: last.map(|r| r.1),
		area_id: area.id,
		active_run_id: rows.first().map(|r| r.run_id),
		queue: rows,
	})
}
pub(crate) async fn cached(access: &mut Access, key: Uuid, digest: &str) -> Result<Option<Value>> {
	// Serialize a tenant/principal key even when concurrent requests target different areas.
	let lock = format!(
		"core:{}:{}:{key}",
		access.identity.tenant, access.identity.subject
	);
	{
		let query_bind_1 = lock;
		sqlx::query(
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
	let previous: Option<(String, Value)> = {
		let query_bind_1 = &access.identity.tenant;
		let query_bind_2 = &access.identity.subject;
		let query_bind_3 = key;
		sqlx::query_as(
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
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("principal"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					),
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
		.fetch_optional(&mut **access.tx)
		.await?
	};
	match previous {
		Some((old, _)) if old != digest => Err(Error::Conflict("IDEMPOTENCY_CONFLICT".into())),
		Some((_, result)) => Ok(Some(result)),
		None => Ok(None),
	}
}
pub(crate) async fn cache(
	access: &mut Access,
	key: Uuid,
	digest: &str,
	result: &Value,
) -> Result<()> {
	sqlx::query(
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

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

#[cfg(test)]
#[path = "../tests/services_sessions_tests.rs"]
mod tests;

use reinhardt::query::SimpleExpr;
