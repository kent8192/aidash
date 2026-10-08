//! Native execution repository preserves caller-owned transactions and lease fences.
use crate::apps::execution::models::{
	Event as EventRecord, HumanRequest as HumanRequestRecord, Invocation as InvocationRecord,
	Run as RunRecord, RunInput as InputRecord, event_records,
};
use crate::apps::execution::serializers::runs::{PeerObservation, RunDetails};
use crate::apps::execution::services::human_interaction;
use crate::apps::federation::remote::models::RemoteRunMessageFence;
use crate::apps::identity::models::AuthorizationRunOutput;
use crate::apps::workspaces::models::{
	Artifact as ArtifactRecord, Message as MessageRecord, Task as TaskRecord,
	Workspace as WorkspaceRecord,
};
use crate::apps::workspaces::serializers::tasks::TaskPage;
use crate::apps::workspaces::services::snapshots;
use crate::database::native::Pool;
use crate::{
	Error, Result,
	domain::*,
	registry::{Entry, Search},
};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::{DatabaseConnection as BackendConnection, TransactionExecutor};
use reinhardt::db::orm::{Model, connection::DatabaseConnectionLease};
use reinhardt::query::ColumnRef;
use reinhardt::query::OnConflict;
use reinhardt::query::SimpleExpr;
use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
use serde_json::{Value, json};
use std::future::Future;
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
	pub capabilities: crate::capabilities::Runtime,
	pub pool: Pool,
	pub control_pool: Pool,
	pub node_id: String,
	pub semantic_client: reqwest::Client,
	pub(crate) recovery_cursors: std::sync::Arc<run_state::RecoveryCursors>,
}

pub use crate::apps::execution::serializers::run_inputs::RunInput;

pub(crate) struct RunResponseMessage<'a> {
	pub run: &'a Run,
	pub worker: uuid::Uuid,
	pub included_input_seq: i64,
	pub sender: &'a str,
	pub content: &'a str,
	pub key: &'a str,
	pub track_output: bool,
}

pub(crate) struct RunMessageDelivery<'a> {
	pub workspace: uuid::Uuid,
	pub task_id: uuid::Uuid,
	pub run_id: uuid::Uuid,
	pub sender: &'a str,
	pub content: &'a str,
	pub input_key: &'a str,
	pub message_key: &'a str,
}

pub(crate) struct FencedRunMessageOutput<'a> {
	pub workspace: uuid::Uuid,
	pub task_id: uuid::Uuid,
	pub run_id: uuid::Uuid,
	pub included_input_seq: i64,
	pub sender: &'a str,
	pub content: &'a str,
	pub key: &'a str,
}

impl Store {
	pub(crate) async fn with_run_response_fence<T, F>(
		&self,
		run_id: Uuid,
		worker: Uuid,
		included_input_seq: i64,
		effect: F,
	) -> Result<T>
	where
		F: Future<Output = Result<T>>,
	{
		let mut tx = self.database().begin().await?;
		RunRecord::ensure_response_current(tx.as_mut(), run_id, worker, included_input_seq).await?;
		let result = effect.await?;
		tx.commit().await?;
		Ok(result)
	}
	pub(crate) async fn response_message_in_run(
		&self,
		request: RunResponseMessage<'_>,
	) -> Result<()> {
		let RunResponseMessage {
			run,
			worker,
			included_input_seq,
			sender,
			content,
			key,
			track_output,
		} = request;
		let mut tx = self.database().begin().await?;
		RunRecord::ensure_response_current(tx.as_mut(), run.id, worker, included_input_seq).await?;
		RunRecord::worker_context(tx.as_mut()).await?;
		nonempty(content, "message")?;
		let message = MessageRecord::append_in(
			tx.as_mut(),
			&self.node_id,
			run.workspace_id,
			sender,
			content,
			key,
		)
		.await?;
		if track_output {
			AuthorizationRunOutput::record(
				tx.as_mut(),
				run.id,
				run.workspace_id,
				"message",
				message.id,
			)
			.await?;
		}
		tx.commit().await?;
		Ok(())
	}

	// Legacy admission cannot supply durable scoped execution authority.
	pub(crate) async fn require_legacy_execution(&self, workspace: Uuid) -> Result<()> {
		let scoped: bool = {
			let query_bind_1 = workspace;
			crate::database::native::query_scalar(
				&Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(EXISTS(SELECT 1 FROM authorization_workspaces WHERE workspace_id = ?))"
							.to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&self.pool)
			.await?
		};
		if scoped {
			return Err(Error::Forbidden);
		}
		Ok(())
	}

	/// Share this store's existing data pool with native persistence operations.
	pub(crate) fn database(&self) -> BackendConnection {
		self.pool.connection()
	}

	/// Borrow this store's existing pool through an owned native ORM scope.
	pub(crate) fn orm_connection(&self) -> Result<DatabaseConnectionLease> {
		Ok(DatabaseConnectionLease::register(self.database())?)
	}

	pub async fn from_pool(pool: sqlx::PgPool, node_id: String) -> Result<Self> {
		let data_pool: Pool = pool.clone().into();
		crate::semantic::repositories::discard::legacy(&data_pool).await?;
		let control_pool = pool
			.options()
			.clone()
			.max_connections(16)
			.connect_with(pool.connect_options().as_ref().clone())
			.await?;
		let memory_recovery = std::env::var_os("AIDASH_MEMORY_RECOVERY_DIR")
			.map(|directory| {
				crate::semantic::repositories::recovery::FileRecovery::new(
					directory.into(),
					node_id.clone(),
				)
			})
			.transpose()?
			.map(|recovery| {
				std::sync::Arc::new(recovery)
					as std::sync::Arc<dyn aidash_application::ports::memory::MemoryRecovery>
			});
		Ok(Self {
			pool: pool.into(),
			control_pool: control_pool.into(),
			node_id,
			semantic_client: crate::semantic::backend::client()?,
			recovery_cursors: Default::default(),
			capabilities: crate::capabilities::Runtime::from_env()?,
		}
		.with_memory_recovery(memory_recovery))
	}

	pub(crate) fn with_memory_recovery(
		mut self,
		recovery: Option<std::sync::Arc<dyn aidash_application::ports::memory::MemoryRecovery>>,
	) -> Self {
		self.pool = self.pool.with_memory_recovery(recovery.clone());
		self.control_pool = self.control_pool.with_memory_recovery(recovery);
		self
	}

	/// Share the database and connection settings without sharing pool capacity.
	pub async fn isolated_pool(&self) -> Result<Self> {
		let mut store = self.worker_pool().await?;
		store.control_pool = self
			.control_pool
			.options()
			.clone()
			.max_connections(4)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.control_pool.connect_options().as_ref().clone())
			.await?
			.into();
		store.control_pool = store
			.control_pool
			.with_memory_recovery(self.control_pool.memory_recovery());
		Ok(store)
	}

	/// Runtime workers reserve data capacity while sharing visibility leases.
	/// Callers must not explicitly close the shared control pool.
	pub async fn worker_pool(&self) -> Result<Self> {
		let pool = self
			.pool
			.options()
			.clone()
			.max_connections(8)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.pool.connect_options().as_ref().clone())
			.await?;
		Ok(Self {
			pool: Pool::from(pool).with_memory_recovery(self.pool.memory_recovery()),
			control_pool: self.control_pool.clone(),
			node_id: self.node_id.clone(),
			semantic_client: self.semantic_client.clone(),
			recovery_cursors: self.recovery_cursors.clone(),
			capabilities: self.capabilities.clone(),
		})
	}

	/// Transaction recovery needs independent control capacity even while
	/// ordinary work retains visibility leases. Other isolated workers share
	/// the node's visibility pool instead of multiplying idle connections.
	pub async fn recovery_pool(&self) -> Result<Self> {
		let pool = self
			.pool
			.options()
			.clone()
			.max_connections(4)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.pool.connect_options().as_ref().clone())
			.await?;
		let control_pool: Pool = self
			.control_pool
			.options()
			.clone()
			.max_connections(4)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.control_pool.connect_options().as_ref().clone())
			.await?
			.into();
		Ok(Self {
			pool: Pool::from(pool).with_memory_recovery(self.pool.memory_recovery()),
			control_pool: control_pool.with_memory_recovery(self.control_pool.memory_recovery()),
			node_id: self.node_id.clone(),
			semantic_client: self.semantic_client.clone(),
			recovery_cursors: self.recovery_cursors.clone(),
			capabilities: self.capabilities.clone(),
		})
	}
	pub async fn event(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<Event> {
		// Sequence allocation and commit order must agree for Last-Event-ID replay.
		crate::database::native::query(
			&Query::select()
				.expr(Expr::cust("PG_ADVISORY_XACT_LOCK(71003201)"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
		Ok({
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = &self.node_id;
			let query_bind_3 = workspace;
			let query_bind_4 = kind;
			let query_bind_5 = data;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("events"))
					.columns([
						Alias::new("id"),
						Alias::new("node_id"),
						Alias::new("workspace_id"),
						Alias::new("kind"),
						Alias::new("data"),
					])
					.from_subquery(
						Query::select()
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
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		})
	}
	pub async fn emit(&self, workspace: Option<Uuid>, kind: &str, data: Value) -> Result<Event> {
		let connection = self.orm_connection()?;
		connection
			.handle()
			.atomic(async |tx| {
				Ok(
					event_records::create(tx, &self.node_id, workspace, kind, data)
						.await?
						.into(),
				)
			})
			.await
	}
	pub async fn create_workspace(&self, title: &str, goal: &str) -> Result<Workspace> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let workspace = self
			.create_workspace_in(&mut tx, Uuid::new_v4(), title, goal)
			.await?;
		tx.commit().await?;
		Ok(workspace)
	}
	pub(crate) async fn create_workspace_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		title: &str,
		goal: &str,
	) -> Result<Workspace> {
		nonempty(title, "title")?;
		nonempty(goal, "goal")?;
		let w: Workspace = {
			let query_bind_1 = id;
			let query_bind_2 = title;
			let query_bind_3 = goal;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("workspaces"))
					.columns([Alias::new("id"), Alias::new("title"), Alias::new("goal")])
					.from_subquery(
						Query::select()
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
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		self.event(tx, Some(w.id), "workspace.created", json!(w))
			.await?;
		Ok(w)
	}
	pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
		let lease = self.orm_connection()?;
		Ok(WorkspaceRecord::newest_first(&mut lease.handle())
			.await?
			.into_iter()
			.map(Into::into)
			.collect())
	}
	pub async fn workspace(&self, id: Uuid) -> Result<Workspace> {
		let lease = self.orm_connection()?;
		Ok(WorkspaceRecord::read(&mut lease.handle(), id).await?.into())
	}
	pub async fn update_state(&self, id: Uuid, revision: i64, state: Value) -> Result<Workspace> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let workspace = self.update_state_in(&mut tx, id, revision, state).await?;
		tx.commit().await?;
		Ok(workspace)
	}
	pub(crate) async fn update_state_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<Workspace> {
		if !state.is_object() {
			return Err(Error::Invalid("workspace state must be an object".into()));
		}
		let w = {
			let query_bind_1 = id;
			let query_bind_2 = revision;
			let query_bind_3 = state;
			aidash_server::database::query_as(
				&Query::update()
					.table(Alias::new("workspaces"))
					.value_expr(
						Alias::new("state"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND revision = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		}
		.ok_or_else(|| Error::Conflict("workspace revision changed".into()))?;
		self.event(tx, Some(id), "workspace.updated", json!(w))
			.await?;
		Ok(w)
	}
	pub async fn task(&self, id: Uuid) -> Result<Task> {
		let lease = self.orm_connection()?;
		Ok(TaskRecord::read(&mut lease.handle(), id).await?.into())
	}
	pub async fn task_page(&self, offset: u64) -> Result<TaskPage> {
		let lease = self.orm_connection()?;
		TaskRecord::page(&mut lease.handle(), offset).await
	}
	pub async fn tasks(&self, workspace: Option<Uuid>) -> Result<Vec<Task>> {
		let lease = self.orm_connection()?;
		Ok(TaskRecord::chronological(&mut lease.handle(), workspace)
			.await?
			.into_iter()
			.map(Into::into)
			.collect())
	}
	pub async fn create_task(
		&self,
		workspace: Uuid,
		input: &NewTask,
		creator: &str,
		key: Option<&str>,
	) -> Result<Task> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let task = self
			.create_task_in(&mut tx, workspace, input, creator, key)
			.await?;
		tx.commit().await?;
		Ok(task)
	}
	pub(crate) async fn create_task_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Uuid,
		input: &NewTask,
		creator: &str,
		key: Option<&str>,
	) -> Result<Task> {
		nonempty(&input.title, "task title")?;
		nonempty(&input.description, "task description")?;
		if !input.requirements.is_object() {
			return Err(Error::Invalid("requirements must be an object".into()));
		}
		let _: Search = serde_json::from_value(input.requirements.clone())
			.map_err(|e| Error::Invalid(e.to_string()))?;
		for dep in input.dependencies.iter().chain(input.parent_id.iter()) {
			let valid: bool = {
				let query_bind_1 = dep;
				let query_bind_2 = workspace;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(EXISTS(SELECT 1 FROM tasks WHERE id = ? AND workspace_id = ?))"
								.to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
			if !valid {
				return Err(Error::Invalid(
					"dependencies and parent must belong to the same workspace".into(),
				));
			}
		}
		let mut ancestor = input.parent_id;
		let mut visited = std::collections::BTreeSet::new();
		while let Some(id) = ancestor {
			if input.dependencies.contains(&id) || !visited.insert(id) {
				return Err(Error::Invalid(
					"task dependencies cannot include an ancestor".into(),
				));
			}
			ancestor = {
				let query_bind_1 = id;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.column(reinhardt::query::Alias::new("parent_id"))
						.from(reinhardt::query::Alias::new("tasks"))
						.and_where(
							reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
								reinhardt::query::Alias::new("id"),
							))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							)),
						)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
		}
		if let Some(parent) = input.parent_id {
			// Completion takes the same row lock before testing its children.
			let status: TaskStatus = {
				let query_bind_1 = parent;
				crate::database::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("status")),
						))
						.from(reinhardt::query::Alias::new("tasks"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Update)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			};
			let replay: bool = {
				let query_bind_1 = key;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(EXISTS(SELECT 1 FROM tasks WHERE creation_key = ?))".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
			if status.is_terminal() && !replay {
				return Err(Error::Conflict(
					"cannot add a child to a terminal parent".into(),
				));
			}
		}
		let task: Option<Task> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = workspace;
			let query_bind_3 = &input.title;
			let query_bind_4 = &input.description;
			let query_bind_5 = &input.requirements;
			let query_bind_6 = creator;
			let query_bind_7 = &input.dependencies;
			let query_bind_8 = input.parent_id;
			let query_bind_9 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("tasks"))
					.columns([
						reinhardt::query::Alias::new("id"),
						reinhardt::query::Alias::new("workspace_id"),
						reinhardt::query::Alias::new("title"),
						reinhardt::query::Alias::new("description"),
						reinhardt::query::Alias::new("requirements"),
						reinhardt::query::Alias::new("created_by"),
						reinhardt::query::Alias::new("dependencies"),
						reinhardt::query::Alias::new("parent_id"),
						reinhardt::query::Alias::new("creation_key"),
					])
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
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![crate::database::uuid_array(query_bind_7.to_owned())],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_8.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_9.to_owned()).into()],
							))
							.to_owned(),
					)
					.on_conflict(
						reinhardt::query::OnConflict::columns([reinhardt::query::Alias::new(
							"creation_key",
						)])
						.do_nothing()
						.to_owned(),
					)
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		let task = match task {
			Some(t) => {
				self.event(tx, Some(workspace), "task.created", json!(t))
					.await?;
				t
			}
			None => {
				let t: Task = {
					let query_bind_1 = key;
					aidash_server::database::query_as(
						&reinhardt::query::Query::select()
							.expr(reinhardt::query::SimpleExpr::from(
								reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
							))
							.from(reinhardt::query::Alias::new("tasks"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(creation_key = ?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.fetch_one(&mut **tx)
					.await?
				};
				if t.workspace_id != workspace
					|| t.title != input.title
					|| t.description != input.description
					|| t.requirements != input.requirements
					|| t.dependencies != input.dependencies
					|| t.parent_id != input.parent_id
					|| t.created_by != creator
				{
					return Err(Error::Conflict(
						"idempotency key reused for a different task".into(),
					));
				}
				t
			}
		};
		Ok(task)
	}
	pub async fn claim(&self, id: Uuid, revision: i64, owner: &str, agent: &Entry) -> Result<Task> {
		let task = self.task(id).await?;
		self.require_legacy_execution(task.workspace_id).await?;
		self.require_legacy_agent(&agent.id, &agent.version).await?;
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let claimed = self
			.claim_in(&mut tx, &task, revision, owner, agent, None)
			.await?;
		crate::semantic::repositories::bindings::claimed(
			self,
			&mut crate::semantic::service::Lease::BorrowedOperator(&mut tx),
			task.id,
			agent,
		)
		.await?;
		tx.commit().await?;
		Ok(claimed)
	}
	pub(crate) async fn claim_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task: &Task,
		revision: i64,
		owner: &str,
		agent: &Entry,
		admitted: Option<&aidash_domain::registry::bindings::BindingSnapshot>,
	) -> Result<Task> {
		let id = task.id;
		let mut requirements: Search = serde_json::from_value(task.requirements.clone())?;
		requirements.kind = Some("agent".into());
		if !requirements.matches(agent) {
			return Err(Error::Invalid(
				"agent does not satisfy task requirements".into(),
			));
		}
		let claimed: Task = { let query_bind_1 = id; let query_bind_2 = revision; let query_bind_3 = owner; aidash_server::database::query_as(&Query::update().table(Alias::new("tasks")).value_expr(Alias::new("status"), Expr::cust("'CLAIMED'")).value_expr(Alias::new("owner"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).value_expr(Alias::new("revision"), Expr::cust("revision + 1")).and_where(SimpleExpr::CustomWithExpr("(id = ? AND revision = ? AND status = 'OPEN' AND NOT EXISTS(SELECT 1 FROM delegations AS d WHERE d.task_id = tasks.id AND d.node_id || '/agents/' || d.agent_id || '@' || d.agent_version <> ?) AND NOT EXISTS(SELECT 1 FROM tasks AS d WHERE d.id = ANY(tasks.dependencies) AND d.status <> 'COMPLETED'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()])).returning_all().to_string(PostgresQueryBuilder)).fetch_optional(&mut **tx).await? }.ok_or_else(|| Error::Conflict("task already claimed, revision changed, or dependencies are incomplete".into()))?;
		if owner == qualified_agent(&self.node_id, &agent.id, &agent.version) {
			let snapshot = if let Some(snapshot) = admitted {
				snapshot.clone()
			} else {
				crate::apps::registry::repositories::bindings::snapshot(
					tx,
					&self.node_id,
					agent,
					false,
				)
				.await?
			};
			snapshot.validate()?;
			let context = crate::context::Context {
				binding_snapshot: Some(Box::new(snapshot)),
				..Default::default()
			};
			// Persist the local execution with the claim; no crash can strand a
			// claimed task between the control API and its worker queue.
			{
				let query_bind_1 = Uuid::new_v4();
				let query_bind_2 = id;
				let query_bind_3 = claimed.workspace_id;
				let query_bind_4 = &self.node_id;
				let query_bind_5 = &agent.id;
				let query_bind_6 = &agent.version;
				let query_bind_7 = serde_json::to_value(&context)?;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("runs"))
						.columns([
							Alias::new("id"),
							Alias::new("task_id"),
							Alias::new("workspace_id"),
							Alias::new("home_node"),
							Alias::new("agent_id"),
							Alias::new("agent_version"),
							Alias::new("context"),
						])
						.from_subquery(
							Query::select()
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
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								))
								.expr(Expr::value(query_bind_7))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns([Alias::new("home_node"), Alias::new("task_id")])
								.do_nothing()
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **tx)
				.await?
			};
			let executor: (String, String) = {
				let query_bind_1 = &self.node_id;
				let query_bind_2 = id;
				crate::database::native::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("agent_id"))))
						.expr(SimpleExpr::from(Expr::col(Alias::new("agent_version"))))
						.from(Alias::new("runs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(home_node = ? AND task_id = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.columns(&["agent_id", "agent_version"])
				.fetch_one(&mut **tx)
				.await?
			};
			if executor != (agent.id.clone(), agent.version.clone()) {
				return Err(Error::Conflict(
					"task has a queued run for a different agent".into(),
				));
			}
		}
		self.event(
			tx,
			Some(claimed.workspace_id),
			"task.claimed",
			json!(claimed),
		)
		.await?;
		Ok(claimed)
	}
	pub async fn transition(
		&self,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
	) -> Result<Task> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let result = self
			.transition_in(&mut tx, id, revision, owner, next)
			.await?;
		tx.commit().await?;
		Ok(result)
	}
	pub(crate) async fn transition_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
	) -> Result<Task> {
		let task: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::ColumnRef::Asterisk)
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		let terminate_unclaimed = matches!(next, TaskStatus::Cancelled | TaskStatus::Failed)
			&& task.status == crate::domain::TaskStatus::Open
			&& task.owner.is_none();
		if task.owner.as_deref() != Some(owner) && !terminate_unclaimed {
			return Err(Error::Unauthorized);
		}
		let before = task.status;
		let after = next;
		if after == TaskStatus::Completed {
			return Err(Error::Invalid(
				"use completion with an idempotency key".into(),
			));
		}
		if task.status == next {
			return Ok(task);
		}
		if !before.can_transition(&after) {
			return Err(Error::Conflict(format!(
				"invalid task transition {} -> {next}",
				task.status
			)));
		}
		let t: Task = { let query_bind_1 = id; let query_bind_2 = revision; let query_bind_3 = owner; let query_bind_4 = next; aidash_server::database::query_as(&reinhardt::query::Query::update().table(reinhardt::query::Alias::new("tasks")).value_expr(reinhardt::query::Alias::new("status"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.as_str()).into()])).value_expr(reinhardt::query::Alias::new("owner"), SimpleExpr::CustomWithExpr("(CASE WHEN ? = 'OPEN' THEN NULL ELSE ? END)".to_owned(), vec![Expr::value(query_bind_4.as_str()).into(), Expr::value(query_bind_3.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1")).and_where(SimpleExpr::CustomWithExpr("(id = ? AND revision = ? AND (owner = ? OR (owner IS NULL AND status = 'OPEN' AND ? IN ('CANCELLED', 'FAILED'))))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.as_str()).into()])).returning_all().to_string(reinhardt::query::PostgresQueryBuilder)).fetch_optional(&mut **tx).await? }.ok_or_else(|| Error::Conflict("task revision changed".into()))?;
		self.event(tx, Some(t.workspace_id), "task.updated", json!(t))
			.await?;
		Ok(t)
	}
	/// Consume this remote run's admitted-message fences and perform an explicit
	/// cancellation or failure while holding the same authoritative task lock.
	pub async fn transition_remote_run_message_terminal(
		&self,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
		run_id: Uuid,
		keys: &[String],
	) -> Result<Task> {
		self.transition_remote_run_message_terminal_in(
			id,
			revision,
			owner,
			next,
			run_id,
			TerminalRunMessageInputs::Keys(keys),
		)
		.await
	}
	pub(crate) async fn transition_remote_run_message_terminal_through(
		&self,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
		run_id: Uuid,
		through_seq: i64,
	) -> Result<Task> {
		if through_seq < 0 {
			return Err(Error::Invalid("invalid terminal input sequence".into()));
		}
		self.transition_remote_run_message_terminal_in(
			id,
			revision,
			owner,
			next,
			run_id,
			TerminalRunMessageInputs::Through(through_seq),
		)
		.await
	}
	async fn transition_remote_run_message_terminal_in(
		&self,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
		run_id: Uuid,
		inputs: TerminalRunMessageInputs<'_>,
	) -> Result<Task> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let result = self
			.transition_remote_terminal_in(&mut tx, id, revision, owner, next, run_id, inputs)
			.await?;
		tx.commit().await?;
		Ok(result)
	}
	/// Explicit operator abandonment preserves the failed outcome and reason
	/// while allowing the parent to finish using the remaining results.
	pub async fn abandon_task(&self, id: Uuid, revision: i64, reason: &str) -> Result<Task> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let task = self
			.abandon_task_in(&mut tx, id, revision, reason, "human")
			.await?;
		tx.commit().await?;
		Ok(task)
	}
	pub(crate) async fn abandon_task_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		revision: i64,
		reason: &str,
		actor: &str,
	) -> Result<Task> {
		nonempty(reason, "abandonment reason")?;
		let task: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if task.revision != revision
			|| !matches!(
				task.status,
				crate::domain::TaskStatus::Failed
					| crate::domain::TaskStatus::Blocked
					| crate::domain::TaskStatus::Cancelled
			) {
			return Err(Error::Conflict(
				"only a failed, blocked or cancelled task at the current revision can be abandoned"
					.into(),
			));
		}
		let active_children: bool = {
			let query_bind_1 = id;
			crate::database::native::query_scalar(&reinhardt::query::Query::select().expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM tasks WHERE parent_id = ? AND NOT status IN ('COMPLETED', 'ABANDONED')))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder)).scalar_one(&mut **tx).await?
		};
		if active_children {
			return Err(Error::Conflict(
				"resolve or abandon this task's children first".into(),
			));
		}
		let updated: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("tasks"))
					.value_expr(
						reinhardt::query::Alias::new("status"),
						reinhardt::query::Expr::cust("'ABANDONED'"),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		self.event(
			tx,
			Some(task.workspace_id),
			"task.abandoned",
			json!({"task":updated,"previous_status":task.status,"reason":reason,"actor":actor}),
		)
		.await?;
		Ok(updated)
	}
	pub async fn complete(
		&self,
		id: Uuid,
		owner: &str,
		key: &str,
		artifact: &ArtifactInput,
	) -> Result<Task> {
		self.complete_from_run(id, owner, key, artifact, None, None)
			.await
	}
	pub(crate) async fn complete_remote_run_message(
		&self,
		id: Uuid,
		owner: &str,
		key: &str,
		artifact: &ArtifactInput,
		run_id: Uuid,
		through_seq: i64,
	) -> Result<Task> {
		if through_seq < 0 {
			return Err(Error::Invalid("invalid terminal input sequence".into()));
		}
		self.complete_from_run(id, owner, key, artifact, None, Some((run_id, through_seq)))
			.await
	}
	pub(crate) async fn complete_from_run(
		&self,
		id: Uuid,
		owner: &str,
		key: &str,
		artifact: &ArtifactInput,
		source_run: Option<Uuid>,
		remote_run_fence: Option<(Uuid, i64)>,
	) -> Result<Task> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let result = self
			.complete_in(
				&mut tx,
				id,
				owner,
				key,
				artifact,
				source_run,
				remote_run_fence,
			)
			.await?;
		tx.commit().await?;
		Ok(result)
	}
	// Preserve the existing fenced store contract while sharing the caller authority transaction.
	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn complete_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		owner: &str,
		key: &str,
		artifact: &ArtifactInput,
		source_run: Option<Uuid>,
		remote_run_fence: Option<(Uuid, i64)>,
	) -> Result<Task> {
		nonempty(key, "idempotency key")?;
		artifact.validate()?;
		let t: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if t.owner.as_deref() != Some(owner) {
			return Err(Error::Unauthorized);
		}
		let existing: Option<Artifact> = {
			let query_bind_1 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("artifacts"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(idempotency_key = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		if let Some(a) = existing {
			if a.task_id != id
				|| a.created_by != owner
				|| a.content != artifact.content
				|| a.kind != artifact.kind
				|| a.name != artifact.name
				|| t.status != crate::domain::TaskStatus::Completed
			{
				return Err(Error::Conflict(
					"completion key reused with different input".into(),
				));
			}
			self.record_output_in(tx, source_run, t.workspace_id, "artifact", a.id)
				.await?;
			return Ok(t);
		}
		if t.status != crate::domain::TaskStatus::Running {
			return Err(Error::Conflict("only a running task can complete".into()));
		}
		let unresolved: bool = {
			let query_bind_1 = id;
			crate::database::native::query_scalar(&reinhardt::query::Query::select().expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM tasks WHERE parent_id = ? AND NOT status IN ('COMPLETED', 'ABANDONED')))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).to_string(reinhardt::query::PostgresQueryBuilder)).scalar_one(&mut **tx).await?
		};
		if unresolved {
			return Err(Error::Conflict("task has unresolved children".into()));
		}
		if let Some((run_id, through_seq)) = remote_run_fence {
			let unobserved: bool = {
				let query_bind_1 = id;
				let query_bind_2 = run_id;
				let query_bind_3 = through_seq;
				crate::database::native::query_scalar(&reinhardt::query::Query::select()
					.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = ? AND NOT consumed AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP) AND (run_id <> ? OR input_seq IS NULL OR input_seq > ?)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
					.to_string(reinhardt::query::PostgresQueryBuilder))
			.scalar_one(&mut **tx)
			.await?
			};
			if unobserved {
				return Err(Error::TransactionPending);
			}
			// Keep these fences through every corrected output write. Consume them
			// atomically with terminal completion so an old worker cannot publish
			// between acknowledgement and task completion.
			{
				let query_bind_1 = id;
				let query_bind_2 = run_id;
				let query_bind_3 = through_seq;
				crate::database::native::query(
					&reinhardt::query::Query::update()
						.table(reinhardt::query::Alias::new("remote_run_message_fences"))
						.value_expr(
							reinhardt::query::Alias::new("consumed"),
							reinhardt::query::Expr::cust("TRUE"),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(task_id = ? AND run_id = ? AND NOT consumed AND input_seq <= ?)"
								.to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
								Expr::value(query_bind_3.to_owned()).into(),
							],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(&mut **tx)
				.await?
			};
		}
		let a: Artifact = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = t.workspace_id;
			let query_bind_3 = id;
			let query_bind_4 = &artifact.kind;
			let query_bind_5 = &artifact.name;
			let query_bind_6 = &artifact.content;
			let query_bind_7 = owner;
			let query_bind_8 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("artifacts"))
					.columns([
						reinhardt::query::Alias::new("id"),
						reinhardt::query::Alias::new("workspace_id"),
						reinhardt::query::Alias::new("task_id"),
						reinhardt::query::Alias::new("kind"),
						reinhardt::query::Alias::new("name"),
						reinhardt::query::Alias::new("content"),
						reinhardt::query::Alias::new("created_by"),
						reinhardt::query::Alias::new("idempotency_key"),
					])
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
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_7.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_8.to_owned()).into()],
							))
							.to_owned(),
					)
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		self.record_output_in(tx, source_run, t.workspace_id, "artifact", a.id)
			.await?;
		let t: Task = {
			let query_bind_1 = id;
			let query_bind_2 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("tasks"))
					.value_expr(
						reinhardt::query::Alias::new("status"),
						reinhardt::query::Expr::cust("'COMPLETED'"),
					)
					.value_expr(
						reinhardt::query::Alias::new("completion_key"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		self.event(
			tx,
			Some(t.workspace_id),
			"task.completed",
			json!({"task":t,"artifact":a}),
		)
		.await?;
		Ok(t)
	}

	pub async fn publish_artifact(
		&self,
		task_id: Uuid,
		owner: &str,
		key: &str,
		input: &ArtifactInput,
	) -> Result<Artifact> {
		self.publish_artifact_from_run(task_id, owner, key, input, None)
			.await
	}
	pub(crate) async fn publish_artifact_from_run(
		&self,
		task_id: Uuid,
		owner: &str,
		key: &str,
		input: &ArtifactInput,
		source_run: Option<Uuid>,
	) -> Result<Artifact> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let result = self
			.publish_artifact_in(&mut tx, task_id, owner, key, input, source_run)
			.await?;
		tx.commit().await?;
		Ok(result)
	}
	pub(crate) async fn publish_artifact_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task_id: Uuid,
		owner: &str,
		key: &str,
		input: &ArtifactInput,
		source_run: Option<Uuid>,
	) -> Result<Artifact> {
		input.validate()?;
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
					.from(Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if task.owner.as_deref() != Some(owner) {
			return Err(Error::Unauthorized);
		}
		let a: Option<Artifact> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = task.workspace_id;
			let query_bind_3 = task_id;
			let query_bind_4 = &input.kind;
			let query_bind_5 = &input.name;
			let query_bind_6 = &input.content;
			let query_bind_7 = owner;
			let query_bind_8 = key;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("artifacts"))
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("task_id"),
						Alias::new("kind"),
						Alias::new("name"),
						Alias::new("content"),
						Alias::new("created_by"),
						Alias::new("idempotency_key"),
					])
					.from_subquery(
						Query::select()
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
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_7.to_owned()).into()],
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_8.to_owned()).into()],
							))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::columns([Alias::new("idempotency_key")])
							.do_nothing()
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		let a = match a {
			Some(a) => {
				self.event(tx, Some(task.workspace_id), "artifact.published", json!(a))
					.await?;
				a
			}
			None => {
				let a: Artifact = {
					let query_bind_1 = key;
					aidash_server::database::query_as(
						&Query::select()
							.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
							.from(Alias::new("artifacts"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(idempotency_key = ?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_one(&mut **tx)
					.await?
				};
				if a.task_id != task_id
					|| a.kind != input.kind
					|| a.name != input.name
					|| a.content != input.content
				{
					return Err(Error::Conflict("artifact idempotency key reused".into()));
				}
				a
			}
		};
		self.record_output_in(tx, source_run, task.workspace_id, "artifact", a.id)
			.await?;
		Ok(a)
	}

	pub async fn events(
		&self,
		after: i64,
		workspace: Option<Uuid>,
		limit: i64,
	) -> Result<Vec<Event>> {
		let lease = self.orm_connection()?;
		Ok(
			EventRecord::after(&mut lease.handle(), after, workspace, limit)
				.await?
				.into_iter()
				.map(Into::into)
				.collect(),
		)
	}
	pub async fn snapshot_page(
		&self,
		workspace: Uuid,
		collection: &str,
		after: Option<Uuid>,
	) -> Result<SnapshotPage> {
		let lease = self.orm_connection()?;
		let rows =
			WorkspaceRecord::snapshot_rows(&mut lease.handle(), workspace, collection, after)
				.await?;
		snapshots::page(rows, after)
	}
	/// Read one exact workspace record without relying on the bounded recent
	/// event/message collections in `snapshot`.
	pub async fn workspace_record(&self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Value> {
		let lease = self.orm_connection()?;
		let db = &mut lease.handle();
		match kind {
			"workspace" if workspace == id => Ok(json!(Workspace::from(
				WorkspaceRecord::read(db, workspace).await?
			))),
			"workspace" => Err(Error::Invalid("workspace record not available".into())),
			"event" => Ok(json!(Event::from(
				EventRecord::in_workspace(db, workspace, id).await?
			))),
			_ => WorkspaceRecord::record(db, workspace, kind, id).await,
		}
	}

	pub async fn child_task_summary(
		&self,
		workspace: Uuid,
		parent: Uuid,
	) -> Result<ChildTaskSummary> {
		let lease = self.orm_connection()?;
		TaskRecord::child_summary(&mut lease.handle(), workspace, parent).await
	}
	pub async fn snapshot(&self, id: Uuid) -> Result<WorkspaceSnapshot> {
		let lease = self.orm_connection()?;
		let db = &mut lease.handle();
		Ok(WorkspaceSnapshot {
			workspace: WorkspaceRecord::read(db, id).await?.into(),
			tasks: TaskRecord::chronological(db, Some(id))
				.await?
				.into_iter()
				.map(Into::into)
				.collect(),
			artifacts: ArtifactRecord::for_workspace(db, id)
				.await?
				.into_iter()
				.map(Into::into)
				.collect(),
			events: EventRecord::recent(db, id)
				.await?
				.into_iter()
				.map(Into::into)
				.collect(),
			messages: MessageRecord::recent(db, id)
				.await?
				.into_iter()
				.map(Into::into)
				.collect(),
		})
	}
	pub async fn message(
		&self,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<()> {
		self.message_record(workspace, sender, content, key)
			.await
			.map(|_| ())
	}
	pub async fn message_record(
		&self,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<Message> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let message = self
			.message_in(&mut tx, workspace, sender, content, key)
			.await?;
		tx.commit().await?;
		Ok(message)
	}
	pub(crate) async fn remote_run_message_history(
		&self,
		workspace: Uuid,
		peer: &str,
		task: Uuid,
		run: Uuid,
		offset: u64,
	) -> Result<Vec<Message>> {
		let lease = self.orm_connection()?;
		MessageRecord::run_history(&mut lease.handle(), workspace, peer, task, run, offset).await
	}
	pub(crate) async fn run_message_delivery_record(
		&self,
		delivery: RunMessageDelivery<'_>,
	) -> Result<Message> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| RemoteRunMessageFence::deliver(tx, &self.node_id, delivery).await)
			.await
	}
	/// Reserve home-task lifetime before the executor admits a remote input.
	/// The task lock orders this operation with every terminal transition.
	pub async fn reserve_remote_run_message(
		&self,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		key: &str,
		content: &str,
	) -> Result<()> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				RemoteRunMessageFence::reserve(tx, task_id, run_id, peer_node, key, content).await
			})
			.await
	}
	/// Persist a reservation beyond its short lease, and bind the admitted input
	/// sequence when one is available. A durable fence remains active until the
	/// matching run atomically completes or transitions the home task.
	pub async fn commit_remote_run_message(
		&self,
		task_id: Uuid,
		run_id: Uuid,
		key: &str,
		content: &str,
	) -> Result<()> {
		self.commit_remote_run_message_with_sequence(task_id, run_id, key, content, None)
			.await
	}
	pub(crate) async fn commit_remote_run_message_with_sequence(
		&self,
		task_id: Uuid,
		run_id: Uuid,
		key: &str,
		content: &str,
		input_seq: Option<i64>,
	) -> Result<()> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				RemoteRunMessageFence::commit(tx, task_id, run_id, key, content, input_seq).await
			})
			.await
	}
	pub async fn release_remote_run_message(
		&self,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		keys: &[String],
	) -> Result<()> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				RemoteRunMessageFence::release(tx, task_id, run_id, peer_node, keys).await
			})
			.await
	}
	pub async fn acknowledge_remote_run_messages(
		&self,
		_task_id: Uuid,
		_run_id: Uuid,
		_keys: &[String],
	) -> Result<()> {
		// Keep the pre-terminal fence even when an older executor reports that
		// it observed the input. Only an atomic terminal transition can make a
		// stale legacy worker unable to publish or complete the home task.
		Ok(())
	}
	pub(crate) async fn run_message_output_record(
		&self,
		workspace: Uuid,
		task_id: Uuid,
		run_id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<Message> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut *tx)
			.await?
		};
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		if matches!(
			task.status,
			crate::domain::TaskStatus::Completed
				| crate::domain::TaskStatus::Failed
				| crate::domain::TaskStatus::Cancelled
				| crate::domain::TaskStatus::Abandoned
		) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		let pending: bool = {
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = ? AND run_id = ? AND NOT consumed AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_one(&mut *tx)
		.await?
		};
		if pending {
			return Err(Error::TransactionPending);
		}
		let message = self
			.message_in(&mut tx, workspace, sender, content, Some(key))
			.await?;
		tx.commit().await?;
		Ok(message)
	}
	/// Publish output from an upgraded remote worker after verifying that its
	/// inference included every currently admitted correction. Keep the fences
	/// unconsumed so legacy workers remain unable to publish until terminal state.
	pub(crate) async fn run_message_output_record_fenced(
		&self,
		output: FencedRunMessageOutput<'_>,
	) -> Result<Message> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let result = self.run_message_output_in(&mut tx, output).await?;
		tx.commit().await?;
		Ok(result)
	}
	pub(crate) async fn run_message_output_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		output: FencedRunMessageOutput<'_>,
	) -> Result<Message> {
		let FencedRunMessageOutput {
			workspace,
			task_id,
			run_id,
			included_input_seq,
			sender,
			content,
			key,
		} = output;
		if included_input_seq < 0 {
			return Err(Error::Invalid("invalid included input sequence".into()));
		}
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		if matches!(
			task.status,
			crate::domain::TaskStatus::Completed
				| crate::domain::TaskStatus::Failed
				| crate::domain::TaskStatus::Cancelled
				| crate::domain::TaskStatus::Abandoned
		) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		let newer_input_pending: bool = {
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = included_input_seq;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = ? AND run_id = ? AND NOT consumed AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP) AND (input_seq IS NULL OR input_seq > ?)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_one(&mut **tx)
		.await?
		};
		if newer_input_pending {
			return Err(Error::TransactionPending);
		}
		let _: String = crate::database::native::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust(
					"set_config('aidash.input_ledger_worker', 'true', true)",
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?;
		let message = self
			.message_in(tx, workspace, sender, content, Some(key))
			.await?;
		Ok(message)
	}
	pub async fn accept_run_message(
		&self,
		run_id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		max_input_tokens: usize,
	) -> Result<()> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				InputRecord::admit(
					tx,
					&self.node_id,
					run_id,
					sender,
					content,
					key,
					max_input_tokens,
				)
				.await
			})
			.await
	}
	pub(crate) async fn accept_run_message_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		run_id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		max_input_tokens: usize,
	) -> Result<()> {
		InputRecord::admit(
			tx,
			&self.node_id,
			run_id,
			sender,
			content,
			key,
			max_input_tokens,
		)
		.await
	}
	pub async fn run_inputs(&self, run_id: Uuid) -> Result<Vec<RunInput>> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| InputRecord::for_run(tx, run_id).await)
			.await
	}
	pub(crate) async fn run_input_sequence(
		&self,
		run_id: Uuid,
		key: &str,
		content: &str,
	) -> Result<i64> {
		let lease = self.orm_connection()?;
		InputRecord::matching_sequence(&mut lease.handle(), run_id, key, content).await
	}
	pub(crate) async fn run_input_high_watermark(&self, run_id: Uuid) -> Result<i64> {
		let lease = self.orm_connection()?;
		InputRecord::high_watermark(&mut lease.handle(), run_id).await
	}
	pub async fn pending_terminal_run_message(&self) -> Result<Option<RunMetadata>> {
		Ok({
			let query_bind_1 = &self.node_id;
			aidash_server::database::query_as(&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr("(home_node <> ? AND phase IN ('COMPLETED', 'FAILED', 'CANCELLED') AND EXISTS (SELECT 1 FROM run_inputs WHERE run_inputs.run_id = runs.id AND message_id IS NULL AND (delivery_retry_at IS NULL OR delivery_retry_at <= CURRENT_TIMESTAMP)))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()]))
				.order_by(reinhardt::query::Alias::new("updated_at"), reinhardt::query::Order::Asc)
				.limit(1)
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(&self.pool)
		.await?
		})
	}

	pub async fn defer_run_message_delivery(&self, run_id: Uuid) -> Result<()> {
		let lease = self.orm_connection()?;
		InputRecord::defer_delivery(&mut lease.handle(), run_id).await
	}
	pub async fn bind_run_input_message(
		&self,
		run_id: Uuid,
		key: &str,
		message_id: Uuid,
	) -> Result<()> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| InputRecord::bind_message(tx, run_id, key, message_id).await)
			.await
	}
	pub async fn import_remote_run_message(
		&self,
		run_id: Uuid,
		key: &str,
		message: &Message,
		max_input_tokens: usize,
	) -> Result<()> {
		self.import_remote_run_messages(
			run_id,
			&[(key.to_owned(), message.clone())],
			max_input_tokens,
		)
		.await
	}
	pub async fn import_remote_run_messages(
		&self,
		run_id: Uuid,
		messages: &[(String, Message)],
		max_input_tokens: usize,
	) -> Result<()> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				InputRecord::import_history(tx, run_id, messages, max_input_tokens).await
			})
			.await
	}
	pub(crate) async fn import_remote_run_messages_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		run_id: Uuid,
		messages: &[(String, Message)],
		max_input_tokens: usize,
	) -> Result<()> {
		InputRecord::import_history(tx, run_id, messages, max_input_tokens).await
	}
	/// Import the remote home's history and new input under one run-row lock.
	pub async fn import_remote_run_messages_and_accept(
		&self,
		run_id: Uuid,
		messages: &[(String, Message)],
		sender: &str,
		content: &str,
		key: &str,
		max_input_tokens: usize,
	) -> Result<()> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				InputRecord::import_history(tx, run_id, messages, max_input_tokens).await?;
				InputRecord::admit(
					tx,
					&self.node_id,
					run_id,
					sender,
					content,
					key,
					max_input_tokens,
				)
				.await
			})
			.await
	}
	pub async fn begin_final_completion(&self, run: &Run, worker: Uuid) -> Result<bool> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let mut current: Run = {
			let query_bind_1 = run.id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("runs"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_until > CURRENT_TIMESTAMP)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?
		}
		.ok_or_else(|| Error::Conflict("worker lease lost".into()))?;
		if current.lease_owner != Some(worker)
			|| current.phase() != crate::domain::RunPhase::ToolCall
			|| current.control == crate::domain::RunControl::Cancelled
			|| current.state.failure_delivery()
		{
			return Err(Error::Conflict("worker lease lost".into()));
		}
		let stale: bool = {
			let query_bind_1 = run.id;
			let query_bind_2 = run.observed_input_seq;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(EXISTS(SELECT 1 FROM run_inputs WHERE run_id = ? AND seq > ?))"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?
		};
		if stale {
			return Ok(false);
		}
		current.state.tool_mut()?.finalizing = true;
		let changed = {
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			let query_bind_3 = current.stored_pending()?;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		};
		if changed.rows_affected() != 1 {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		tx.commit().await?;
		Ok(true)
	}
	pub(crate) async fn message_from_run(
		&self,
		run: &Run,
		sender: &str,
		content: &str,
		key: &str,
	) -> Result<()> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let message = self
			.message_in(&mut tx, run.workspace_id, sender, content, Some(key))
			.await?;
		self.record_output_in(
			&mut tx,
			Some(run.id),
			run.workspace_id,
			"message",
			message.id,
		)
		.await?;
		tx.commit().await?;
		Ok(())
	}
	pub(crate) async fn record_output_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run: Option<Uuid>,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		let Some(run) = run else {
			return Ok(());
		};
		let valid: bool = {
			let query_bind_1 = run;
			let query_bind_2 = workspace;
			crate::database::native::query_scalar(&Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM authorization_execution WHERE run_id = ? AND workspace_id = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.to_string(PostgresQueryBuilder))
		.scalar_one(&mut **tx)
		.await?
		};
		if !valid {
			return Err(Error::Forbidden);
		}
		{
			let query_bind_1 = run;
			let query_bind_2 = workspace;
			let query_bind_3 = kind;
			let query_bind_4 = id;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("authorization_run_reads"))
					.columns([
						Alias::new("run_id"),
						Alias::new("workspace_id"),
						Alias::new("resource_kind"),
						Alias::new("resource_id"),
					])
					.from_subquery(
						Query::select()
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
							.to_owned(),
					)
					.on_conflict(
						OnConflict::columns(["run_id", "resource_kind", "resource_id"])
							.do_nothing()
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
		{
			let query_bind_1 = run;
			let query_bind_2 = workspace;
			let query_bind_3 = kind;
			let query_bind_4 = id;
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("authorization_run_outputs"))
					.columns([
						Alias::new("run_id"),
						Alias::new("workspace_id"),
						Alias::new("resource_kind"),
						Alias::new("resource_id"),
					])
					.from_subquery(
						Query::select()
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
							.to_owned(),
					)
					.on_conflict(
						OnConflict::columns(["run_id", "resource_kind", "resource_id"])
							.do_nothing()
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};

		Ok(())
	}
	pub(crate) async fn message_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<Message> {
		nonempty(content, "message")?;
		self.message_in_with_attachments(tx, workspace, sender, content, key)
			.await
	}

	/// The channel may carry a media-only message. The caller must check that
	/// at least one attachment belongs to this submission before calling it.
	pub(crate) async fn message_in_with_attachments(
		&self,
		tx: &mut crate::database::native::Transaction,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<Message> {
		let inserted: Option<Message> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = workspace;
			let query_bind_3 = sender;
			let query_bind_4 = content;
			let query_bind_5 = key;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("messages"))
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("sender"),
						Alias::new("content"),
						Alias::new("idempotency_key"),
					])
					.from_subquery(
						Query::select()
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
					.on_conflict(
						OnConflict::columns([Alias::new("idempotency_key")])
							.do_nothing()
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		let message = if let Some(message) = inserted {
			self.event(tx, Some(workspace), "message.created", json!(message))
				.await?;
			message
		} else {
			let message: Message = {
				let query_bind_1 = key;
				aidash_server::database::query_as(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
						.from(Alias::new("messages"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(idempotency_key = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			};
			if message.workspace_id != workspace
				|| message.sender != sender
				|| message.content != content
			{
				return Err(Error::Conflict("message idempotency key reused".into()));
			}
			message
		};
		Ok(message)
	}
}

impl Store {
	pub(crate) async fn require_legacy_agent(&self, id: &str, version: &str) -> Result<()> {
		let lease = self.orm_connection()?;
		let mut db = lease.handle();
		// Legacy peer journals can retain references absent from the local Registry.
		// Reject known scoped projections without requiring a local definition.
		let installed =
			match crate::apps::registry::models::records::definition(&mut db, id, version).await {
				Ok(row) => row.metadata.0.get("installation").is_some(),
				Err(Error::NotFound(_)) => false,
				Err(error) => return Err(error),
			};
		if installed {
			return Err(Error::Forbidden);
		}

		let generated: bool = {
			let query_bind_1 = id;
			let query_bind_2 = version;
			crate::database::native::query_scalar(&Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM generation_requests WHERE agent_id = ? AND agent_version = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.to_string(PostgresQueryBuilder))
		.scalar_one(&self.pool)
		.await?
		};
		if generated {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	pub(crate) async fn require_legacy_remote_task(&self, home: &str, task: Uuid) -> Result<()> {
		let admitted: bool = {
			let query_bind_1 = home;
			let query_bind_2 = task;
			crate::database::native::query_scalar(&Query::select().expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM authorization_remote_admissions WHERE source_node = ? AND task_id = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()])).to_string(PostgresQueryBuilder)).scalar_one(&self.pool).await?
		};
		if admitted {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	pub async fn accept_run(
		&self,
		task: &Task,
		home_node: &str,
		agent_id: &str,
		agent_version: &str,
	) -> Result<Run> {
		self.accept_run_pinned(task, home_node, agent_id, agent_version, None)
			.await
	}
	pub(crate) async fn accept_run_pinned(
		&self,
		task: &Task,
		home_node: &str,
		agent_id: &str,
		agent_version: &str,
		pinned: Option<&aidash_domain::registry::bindings::ForeignAgentSnapshot>,
	) -> Result<Run> {
		if let Some(pinned) = pinned {
			pinned.validate()?;
			if pinned.agent.registry_node != self.node_id
				|| pinned.agent.id != agent_id
				|| pinned.agent.version != agent_version
				|| home_node == self.node_id
			{
				return Err(Error::Forbidden);
			}
		}
		self.require_legacy_execution(task.workspace_id).await?;
		let mut tx = crate::database::native::begin(&self.pool).await?;
		// Serialize legacy admission against scoped receiver admission. Neither
		// mode may appear between the other's check and durable commit.
		{
			let query_bind_1 = format!("{home_node}:{}", task.id);
			crate::database::native::query(
				&Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED(?, 71003209)))".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		};
		let admitted: bool = {
			let query_bind_1 = home_node;
			let query_bind_2 = task.id;
			crate::database::native::query_scalar(&Query::select().expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM authorization_remote_admissions WHERE source_node = ? AND task_id = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()])).to_string(PostgresQueryBuilder)).scalar_one(&mut *tx).await?
		};
		if admitted {
			return Err(Error::Forbidden);
		}
		let existing: Option<Run> = aidash_server::database::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("runs"))
				.and_where(Expr::col(Alias::new("home_node")).eq(home_node))
				.and_where(Expr::col(Alias::new("task_id")).eq(Expr::value(task.id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut *tx)
		.await?;
		if let Some(run) = existing {
			if run.agent_id != agent_id
				|| run.agent_version != agent_version
				|| run.workspace_id != task.workspace_id
				|| pinned.is_some_and(|snapshot| {
					run.context.binding_snapshot.as_deref() != Some(&snapshot.snapshot())
				}) {
				return Err(Error::Conflict(
					"task already has a different executor".into(),
				));
			}
			tx.commit().await?;
			return Ok(run);
		}
		self.require_legacy_agent(agent_id, agent_version).await?;
		let agent = aidash_application::ports::registry::DefinitionLookup::definition(
			&mut crate::apps::registry::repositories::sql::SqlScope(&mut tx),
			agent_id,
			agent_version,
		)
		.await?;
		let snapshot = crate::apps::registry::repositories::bindings::snapshot(
			&mut tx,
			&self.node_id,
			&agent,
			home_node != self.node_id,
		)
		.await?;
		if pinned.is_some_and(|pinned| pinned.snapshot() != snapshot) {
			return Err(Error::Conflict(
				"offered Agent closure differs from receiver admission".into(),
			));
		}
		let context = crate::context::Context {
			binding_snapshot: Some(Box::new(snapshot)),
			..Default::default()
		};
		let row: Option<Run> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = task.id;
			let query_bind_3 = task.workspace_id;
			let query_bind_4 = home_node;
			let query_bind_5 = agent_id;
			let query_bind_6 = agent_version;
			let query_bind_7 = serde_json::to_value(&context)?;
			aidash_server::database::query_as(
				&Query::insert()
					.into_table(Alias::new("runs"))
					.columns([
						Alias::new("id"),
						Alias::new("task_id"),
						Alias::new("workspace_id"),
						Alias::new("home_node"),
						Alias::new("agent_id"),
						Alias::new("agent_version"),
						Alias::new("context"),
					])
					.from_subquery(
						Query::select()
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
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.expr(Expr::value(query_bind_7))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::columns([Alias::new("home_node"), Alias::new("task_id")])
							.do_nothing()
							.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?
		};
		let run = match row {
			Some(r) => {
				self.event(
					&mut tx,
					(home_node == self.node_id).then_some(task.workspace_id),
					"run.created",
					json!(r),
				)
				.await?;
				r
			}
			None => {
				let r: Run = {
					let query_bind_1 = home_node;
					let query_bind_2 = task.id;
					aidash_server::database::query_as(
						&Query::select()
							.expr(SimpleExpr::from(Expr::col(ColumnRef::Asterisk)))
							.from(Alias::new("runs"))
							.and_where(SimpleExpr::CustomWithExpr(
								"(home_node = ? AND task_id = ?)".to_owned(),
								vec![
									Expr::value(query_bind_1.to_owned()).into(),
									Expr::value(query_bind_2.to_owned()).into(),
								],
							))
							.to_string(PostgresQueryBuilder),
					)
					.fetch_one(&mut *tx)
					.await?
				};
				if r.agent_id != agent_id
					|| r.agent_version != agent_version
					|| r.workspace_id != task.workspace_id
				{
					return Err(Error::Conflict(
						"task already has a different executor".into(),
					));
				}
				r
			}
		};
		if home_node == self.node_id {
			// Legacy journals may name an Agent absent from this Registry.
			// Such runs have no native-memory configuration to bind; known
			// scoped/generated Agents are still rejected by require_legacy_agent.
			match crate::apps::registry::models::Definition::read_in(
				&mut *tx,
				agent_id,
				agent_version,
			)
			.await
			{
				Ok(entry) => {
					crate::semantic::repositories::bindings::admit(
						self,
						&mut crate::semantic::service::Lease::BorrowedOperator(&mut tx),
						&run.metadata(),
						&entry,
					)
					.await?;
				}
				Err(Error::NotFound(_)) => {}
				Err(error) => return Err(error),
			}
		}
		tx.commit().await?;
		Ok(run)
	}
	pub async fn run(&self, id: Uuid) -> Result<Run> {
		let lease = self.orm_connection()?;
		RunRecord::read(&mut lease.handle(), id).await?.try_into()
	}
	/// List validated executable records. Inspection uses inspect_runs and retains invalid rows.
	pub async fn runs(&self) -> Result<Vec<Run>> {
		let rows: Vec<RawRun> = aidash_server::database::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.order_by_expr(
					reinhardt::query::SimpleExpr::from(reinhardt::query::Expr::col(
						reinhardt::query::Alias::new("updated_at"),
					)),
					reinhardt::query::Order::Desc,
				)
				.limit(500)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_all(&self.pool)
		.await?;
		Ok(rows
			.into_iter()
			.filter_map(|raw| raw.decode().ok())
			.collect())
	}
	pub async fn lease_run(&self, worker: Uuid, seconds: i32) -> Result<Option<Run>> {
		let mut tx = self.database().begin().await?;
		let mut cursor = self.recovery_cursors.execution.lock().await;
		let run = Self::lease_run_in(
			tx.as_mut(),
			worker,
			seconds,
			None,
			&self.node_id,
			&mut cursor,
		)
		.await?;
		tx.commit().await?;
		Ok(run)
	}
	pub async fn renew_lease(&self, id: Uuid, worker: Uuid, seconds: i32) -> Result<bool> {
		let mut tx = self.database().begin().await?;
		let renewed = RunRecord::renew(tx.as_mut(), id, worker, seconds).await?;
		tx.commit().await?;
		Ok(renewed)
	}

	pub async fn save_run(&self, run: &Run, worker: Uuid, kind: &str) -> Result<Run> {
		let aidash_domain::run_state::persistence::WorkerSnapshot { pending, error } =
			run.worker_snapshot(kind)?;
		let mut tx = crate::database::native::begin(&self.pool).await?;
		if kind == "model.completed" {
			// Message admission takes this same row lock. A response is durable only
			// when every accepted input was present in its provider request.
			let _: Uuid = {
				let query_bind_1 = run.id;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.column(reinhardt::query::Alias::new("id"))
						.from(reinhardt::query::Alias::new("runs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Update)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut *tx)
				.await?
			};
			let stale: bool = {
				let query_bind_1 = run.id;
				let query_bind_2 = run.included_input_seq();
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(EXISTS(SELECT 1 FROM run_inputs WHERE run_id = ? AND seq > ?))"
								.to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_one(&mut *tx)
				.await?
			};
			if stale {
				return Err(Error::StaleInference);
			}
		}
		let saved: Run = { let query_bind_1 = run.id; let query_bind_2 = worker; let query_bind_3 = run.phase(); let query_bind_4 = &run.context; let query_bind_5 = &pending; let query_bind_6 = run.step; let query_bind_7 = error; let query_bind_8 = kind != "model.completed"; let query_bind_9 = run.observed_input_seq; aidash_server::database::query_as(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("phase"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.as_str()).into()])).value_expr(reinhardt::query::Alias::new("context"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(serde_json::to_value(query_bind_4)?).into()])).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("step"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("error"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_7.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1")).value_expr(reinhardt::query::Alias::new("observed_input_seq"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_9.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("updated_at"), reinhardt::query::Expr::cust("CURRENT_TIMESTAMP")).value_expr(reinhardt::query::Alias::new("lease_owner"), reinhardt::query::Expr::cust(if kind == "run.sources_observed" { "lease_owner" } else { "NULL" })).value_expr(reinhardt::query::Alias::new("lease_until"), reinhardt::query::Expr::cust(if kind == "run.sources_observed" { "lease_until" } else { "NULL" }))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP AND (? OR control <> 'CANCELLED'))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_8.to_owned()).into()]))
				.returning_all()
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(&mut *tx)
		.await? }
		.ok_or_else(|| Error::Conflict("worker lease lost".into()))?;
		self.event(&mut tx, (run.home_node == self.node_id).then_some(run.workspace_id), kind,
            json!({"run_id":saved.id,"task_id":saved.task_id,"workspace_id":saved.workspace_id,"agent_id":saved.agent_id,"phase":saved.phase(),"step":saved.step,"error":saved.error,"context_usage":saved.context.usage})).await?;
		tx.commit().await?;
		Ok(saved)
	}
	pub async fn release_lease(&self, id: Uuid, worker: Uuid) -> Result<()> {
		let mut tx = self.database().begin().await?;
		RunRecord::release_worker(tx.as_mut(), id, worker).await?;
		tx.commit().await?;
		Ok(())
	}
	pub(crate) async fn pause_for_execution(
		&self,
		run: impl Into<RunMetadata>,
		worker: Uuid,
		reason: &str,
		event_kind: &str,
	) -> Result<()> {
		self.pause_for_execution_reason(run, worker, reason, event_kind, None)
			.await
	}
	pub(crate) async fn cancel_execution(&self, run: &Run, worker: Uuid) -> Result<()> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let valid: Option<Uuid> = {
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("id")),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP AND control = 'CANCELLED')".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.lock(reinhardt::query::LockType::Update)
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_optional(&mut *tx)
		.await?
		};
		if valid.is_none() {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		let task: Task = {
			let query_bind_1 = run.task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut *tx)
			.await?
		};
		let phase = if task.status == crate::domain::TaskStatus::Completed {
			"COMPLETED"
		} else {
			"CANCELLED"
		};
		if !matches!(
			task.status,
			crate::domain::TaskStatus::Completed
				| crate::domain::TaskStatus::Cancelled
				| crate::domain::TaskStatus::Abandoned
		) {
			let task: Task = {
				let query_bind_1 = run.task_id;
				aidash_server::database::query_as(
					&reinhardt::query::Query::update()
						.table(reinhardt::query::Alias::new("tasks"))
						.value_expr(
							reinhardt::query::Alias::new("status"),
							reinhardt::query::Expr::cust("'CANCELLED'"),
						)
						.value_expr(
							reinhardt::query::Alias::new("revision"),
							reinhardt::query::Expr::cust("revision + 1"),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.returning_all()
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(&mut *tx)
				.await?
			};
			self.event(&mut tx, Some(run.workspace_id), "task.updated", json!(task))
				.await?;
		}
		{
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			let query_bind_3 = phase;
			let query_bind_4 = crate::domain::run_state::encode(
				&if phase == "COMPLETED" {
					RunState::Completed(TerminalState {})
				} else {
					RunState::Cancelled(TerminalState {})
				},
				&RecoveryState::default(),
			)?;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("phase"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("error"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.value_expr(
						reinhardt::query::Alias::new("updated_at"),
						reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"),
					)
					.value_expr(
						reinhardt::query::Alias::new("lease_owner"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.value_expr(
						reinhardt::query::Alias::new("lease_until"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		};
		self.event(
			&mut tx,
			Some(run.workspace_id),
			if phase == "CANCELLED" {
				"run.cancelled"
			} else {
				"run.reconciled"
			},
			json!({"run_id":run.id,"task_id":run.task_id}),
		)
		.await?;
		tx.commit().await?;
		Ok(())
	}

	pub async fn control(&self, id: Uuid, action: RunControlAction) -> Result<RunInspection> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let run = self.control_in(&mut tx, id, action).await?;
		tx.commit().await?;
		Ok(run)
	}

	pub(crate) async fn control_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		action: RunControlAction,
	) -> Result<RunInspection> {
		let control = action.control();
		let mut resumed_pending = None;
		if action == RunControlAction::Resume {
			let raw: RawRun = {
				let query_bind_1 = id;
				aidash_server::database::query_as(
					&reinhardt::query::Query::select()
						.column(reinhardt::query::ColumnRef::Asterisk)
						.from(reinhardt::query::Alias::new("runs"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Update)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			};
			resumed_pending = raw.resumed_pending()?;
		}
		// Control-plane updates are emitted by upgraded code and must remain
		// available while a pre-upgrade worker lease is fenced by run_inputs.
		crate::database::native::query_scalar::<String>(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust(
					"set_config('aidash.input_ledger_worker', 'true', true)",
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?;
		let r: RawRun = { let query_bind_1 = id; let query_bind_2 = control; let query_bind_3 = resumed_pending; aidash_server::database::query_as(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(COALESCE(?::jsonb, pending))".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("control"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.as_str()).into()])).value_expr(reinhardt::query::Alias::new("error"), SimpleExpr::CustomWithExpr("(CASE WHEN ?::jsonb IS NOT NULL OR (? = 'PAUSED' AND error = 'identity status unavailable') THEN NULL ELSE error END)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_2.as_str()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1")).value_expr(reinhardt::query::Alias::new("updated_at"), reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND NOT phase IN ('COMPLETED', 'FAILED', 'CANCELLED') AND control <> 'CANCELLED')".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()]))
				.returning_all()
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(&mut **tx)
		.await? }
		.ok_or_else(|| {
			Error::Conflict("run is terminal or cancellation is already requested".into())
		})?;
		self.event(
			tx,
			(r.metadata.home_node == self.node_id).then_some(r.metadata.workspace_id),
			"run.control",
			json!({"run_id":id,"action":action}),
		)
		.await?;
		Ok(r.inspect())
	}

	pub async fn human_request(
		&self,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		let mut tx = self.database().begin().await?;
		let request = self
			.human_request_in(tx.as_mut(), run, kind, prompt, key)
			.await?;
		tx.commit().await?;
		Ok(request)
	}

	pub(crate) async fn cache_home_human(
		&self,
		run: &Run,
		request: &HumanRequest,
		key: &str,
	) -> Result<()> {
		if request.run_id != run.id || request.workspace_id != run.workspace_id {
			return Err(Error::Forbidden);
		}
		let mut tx = self.database().begin().await?;
		let (cached, created) = HumanRequestRecord::admit_home(tx.as_mut(), request, key).await?;
		human_interaction::validate_replay(&cached, run, &request.kind, &request.prompt)?;
		if cached.id != request.id {
			return Err(Error::Conflict(
				"Home human request identity changed".into(),
			));
		}
		if created {
			event_records::append(
				tx.as_mut(),
				&self.node_id,
				None,
				"human.requested",
				json!(request),
			)
			.await?;
		}
		tx.commit().await?;
		Ok(())
	}

	async fn human_request_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		human_interaction::validate_request(kind, prompt)?;
		let (request, created) =
			HumanRequestRecord::admit(tx, run.workspace_id, run.id, kind, prompt, key).await?;
		human_interaction::validate_replay(&request, run, kind, prompt)?;
		if created {
			event_records::append(
				tx,
				&self.node_id,
				(run.home_node == self.node_id).then_some(run.workspace_id),
				"human.requested",
				json!(request),
			)
			.await?;
		}
		Ok(request)
	}

	pub(crate) async fn reconciliation_request(
		&self,
		run: &mut Run,
		worker: Uuid,
		key: &str,
		prompt: &str,
	) -> Result<()> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let h = self
			.human_request_sqlx_in(
				&mut tx,
				run,
				"CONFIRMATION",
				prompt,
				&format!("{key}:reconcile"),
			)
			.await?;
		run.state = RunState::Waiting(Box::new(WaitingState::Reconciliation {
			request_id: h.id,
			key: key.into(),
			resume: Box::new(run.state.tool()?.clone()),
		}));
		let changed = {
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			let query_bind_3 = run.stored_pending()?;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value_expr(
						reinhardt::query::Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("phase"),
						reinhardt::query::Expr::cust("'WAITING'"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		}
		.rows_affected();
		if changed != 1 {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		tx.commit().await?;
		Ok(())
	}

	pub(crate) async fn human_request_by_id(&self, id: Uuid) -> Result<HumanRequest> {
		let mut tx = self.database().begin().await?;
		let request = HumanRequestRecord::read_in(tx.as_mut(), id, false)
			.await?
			.ok_or_else(|| Error::NotFound("human request".into()))?;
		tx.commit().await?;
		Ok(request)
	}

	pub(crate) async fn expire_workbench_approval(&self, id: Uuid) -> Result<HumanRequest> {
		let mut tx = self.database().begin().await?;
		let request = HumanRequestRecord::read_in(tx.as_mut(), id, true)
			.await?
			.ok_or_else(|| Error::NotFound("human request".into()))?;
		let result = if human_interaction::approval_expired(&request, chrono::Utc::now()) {
			self.answer_in(
				tx.as_mut(),
				id,
				json!({"approved": false, "expired": true}),
				"system",
			)
			.await?
		} else {
			request
		};
		tx.commit().await?;
		Ok(result)
	}

	pub async fn answer(&self, id: Uuid, response: Value) -> Result<HumanRequest> {
		let mut tx = self.database().begin().await?;
		let request = self.answer_in(tx.as_mut(), id, response, "human").await?;
		tx.commit().await?;
		Ok(request)
	}

	pub(crate) async fn answer_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		response: Value,
		actor: &str,
	) -> Result<HumanRequest> {
		human_interaction::validate_response(&response)?;
		let old = HumanRequestRecord::read_in(tx, id, true)
			.await?
			.ok_or_else(|| Error::NotFound("human request".into()))?;
		let run = RunRecord::read_in(tx, old.run_id, None)
			.await?
			.ok_or_else(|| Error::NotFound("run".into()))?;
		let human_interaction::Answer::Write { response, actor } =
			human_interaction::answer(&old, &run, response, actor, chrono::Utc::now())?
		else {
			return Ok(old);
		};
		let request = HumanRequestRecord::answer_locked(tx, id, response, &actor).await?;
		event_records::append(
			tx,
			&self.node_id,
			(run.home_node == self.node_id).then_some(run.workspace_id),
			"human.answered",
			json!(request),
		)
		.await?;
		Ok(request)
	}
	pub async fn invocation_start(
		&self,
		run: &Run,
		worker: Uuid,
		key: &str,
		tool: &str,
		input: &Value,
		replay_safe: bool,
	) -> Result<Invocation> {
		let mut tx = crate::database::native::begin(&self.pool).await?;
		self.ensure_run_response_current_in(&mut tx, run.id, worker, run.included_input_seq())
			.await?;
		// The bounded call and any prepared workspace-read chunk must survive a
		// worker crash once the idempotency key becomes durable. Keep this update
		// and its derived phase in the same transaction as invocation creation.
		let persisted = {
			let query_bind_1 = run.id;
			let query_bind_2 = worker;
			let query_bind_3 = run.stored_pending()?;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("runs"))
					.value(reinhardt::query::Alias::new("phase"), run.phase().as_str())
					.value_expr(
						reinhardt::query::Alias::new("pending"),
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					)
					.value_expr(
						reinhardt::query::Alias::new("revision"),
						reinhardt::query::Expr::cust("revision + 1"),
					)
					.value_expr(
						reinhardt::query::Alias::new("updated_at"),
						reinhardt::query::Expr::cust("CURRENT_TIMESTAMP"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.and_where(
						reinhardt::query::Expr::col(reinhardt::query::Alias::new("phase"))
							.is_not_in([
								RunPhase::Completed.as_str(),
								RunPhase::Failed.as_str(),
								RunPhase::Cancelled.as_str(),
							]),
					)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?
		}
		.rows_affected();
		if persisted != 1 {
			return Err(Error::Conflict(
				"worker lease lost before persisting tool input".into(),
			));
		}
		let created = {
			let query_bind_1 = key;
			let query_bind_2 = run.id;
			let query_bind_3 = tool;
			let query_bind_4 = input;
			let query_bind_5 = replay_safe;
			crate::database::native::query(&format!(
				"{} ON CONFLICT DO NOTHING",
				reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("invocations"))
					.columns([
						reinhardt::query::Alias::new("idempotency_key"),
						reinhardt::query::Alias::new("run_id"),
						reinhardt::query::Alias::new("tool"),
						reinhardt::query::Alias::new("input"),
						reinhardt::query::Alias::new("status"),
						reinhardt::query::Alias::new("replay_safe"),
					])
					.from_subquery(
						reinhardt::query::Query::select()
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()]
							))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_4.to_owned()).into()]
							))
							.expr(reinhardt::query::Expr::cust("'STARTED'"))
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_5.to_owned()).into()]
							))
							.to_owned()
					)
					.to_owned()
					.to_string(reinhardt::query::PostgresQueryBuilder)
			))
			.execute(&mut *tx)
			.await?
		}
		.rows_affected()
			== 1;
		let mut invocation: Invocation = {
			let query_bind_1 = key;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::SimpleExpr::from(
						reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
					))
					.from(reinhardt::query::Alias::new("invocations"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(idempotency_key = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut *tx)
			.await?
		};
		if invocation.input != *input || invocation.tool != tool || invocation.run_id != run.id {
			return Err(Error::Conflict(
				"tool idempotency key reused with different input".into(),
			));
		}
		if !created && invocation.status != "COMPLETED" && !invocation.replay_safe {
			{
				let query_bind_1 = key;
				crate::database::native::query(
					&reinhardt::query::Query::update()
						.table(reinhardt::query::Alias::new("invocations"))
						.value_expr(
							reinhardt::query::Alias::new("status"),
							reinhardt::query::Expr::cust("'UNCERTAIN'"),
						)
						.and_where(SimpleExpr::CustomWithExpr(
							"(idempotency_key = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(&mut *tx)
				.await?
			};
			invocation.status = "UNCERTAIN".into();
		}
		if created {
			self.event(
				&mut tx,
				(run.home_node == self.node_id).then_some(run.workspace_id),
				"tool.started",
				json!({"run_id":run.id,"tool":tool,"idempotency_key":key,"input":input}),
			)
			.await?;
		}
		tx.commit().await?;
		Ok(invocation)
	}
	pub async fn invocation_finish(
		&self,
		run: &Run,
		worker: Uuid,
		key: &str,
		result: &Value,
	) -> Result<()> {
		let mut tx = self.database().begin().await?;
		if !RunRecord::hold_worker(tx.as_mut(), run.id, worker).await? {
			return Err(Error::Conflict(
				"worker lease lost while recording tool result".into(),
			));
		}
		if InvocationRecord::complete(tx.as_mut(), run.id, key, result).await? {
			event_records::append(
				tx.as_mut(),
				&self.node_id,
				(run.home_node == self.node_id).then_some(run.workspace_id),
				"tool.completed",
				json!({"run_id":run.id,"idempotency_key":key,"result":result}),
			)
			.await?;
		}
		tx.commit().await?;
		Ok(())
	}
	// The empty namespace preserves pre-federation local memory. Peer node IDs
	// are validated nonempty, so no remote home can address this namespace.
	pub(crate) async fn peer_observation(&self, home: &str) -> Result<PeerObservation> {
		let lease = self.orm_connection()?;
		crate::apps::execution::models::journals::observe(&mut lease.handle(), &self.node_id, home)
			.await
	}
	pub(crate) async fn run_details_in(
		&self,
		tx: &mut dyn TransactionExecutor,
		run: Run,
		offset: u64,
	) -> Result<RunDetails> {
		let invocations = InvocationRecord::page(tx, run.id, offset).await?;
		let memory = crate::semantic::repositories::bindings::load(tx, &run.metadata()).await?;
		Ok(RunDetails {
			media_input_routes: Vec::new(),
			run: run.into(),
			invocations,
			memory,
		})
	}
	pub(crate) async fn run_details(&self, id: Uuid, offset: u64) -> Result<RunDetails> {
		let lease = self.orm_connection()?;
		lease
			.handle()
			.atomic(async |tx| {
				let run = RunRecord::objects()
					.filter(RunRecord::field_id().eq(id))
					.all_with_executor(tx)
					.await
					.map_err(FrameworkError::from)?
					.pop()
					.ok_or_else(|| Error::NotFound("run".into()))?;
				self.run_details_in(tx, run.try_into()?, offset).await
			})
			.await
	}
	#[allow(clippy::too_many_arguments)]
	pub(crate) async fn transition_remote_terminal_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: TaskStatus,
		run_id: Uuid,
		inputs: TerminalRunMessageInputs<'_>,
	) -> Result<Task> {
		if !matches!(next, TaskStatus::Cancelled | TaskStatus::Failed) {
			return Err(Error::Invalid(
				"remote run-message terminal transition must be cancelled or failed".into(),
			));
		}
		let task: Task = {
			let query_bind_1 = id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		let terminate_unclaimed = task.status == crate::domain::TaskStatus::Open
			&& task.owner.is_none()
			&& matches!(next, TaskStatus::Cancelled | TaskStatus::Failed);
		if task.owner.as_deref() != Some(owner) && !terminate_unclaimed {
			return Err(Error::Unauthorized);
		}
		if task.revision != revision {
			return Err(Error::Conflict("task revision changed".into()));
		}
		if task.status != next {
			let before = task.status;
			let after = next;
			if !before.can_transition(&after) {
				return Err(Error::Conflict(format!(
					"invalid task transition {} -> {next}",
					task.status
				)));
			}
		}
		match inputs {
			TerminalRunMessageInputs::Keys(keys) => {
				for key in keys {
					{
						let query_bind_1 = id;
						let query_bind_2 = run_id;
						let query_bind_3 = key;
						crate::database::native::query(
							&reinhardt::query::Query::update()
								.table(reinhardt::query::Alias::new("remote_run_message_fences"))
								.value_expr(
									reinhardt::query::Alias::new("consumed"),
									reinhardt::query::Expr::cust("TRUE"),
								)
								.and_where(SimpleExpr::CustomWithExpr(
									"(task_id = ? AND run_id = ? AND idempotency_key = ?)"
										.to_owned(),
									vec![
										Expr::value(query_bind_1.to_owned()).into(),
										Expr::value(query_bind_2.to_owned()).into(),
										Expr::value(query_bind_3.to_owned()).into(),
									],
								))
								.to_string(reinhardt::query::PostgresQueryBuilder),
						)
						.execute(&mut **tx)
						.await?
					};
				}
			}
			TerminalRunMessageInputs::Through(through_seq) => {
				// Only a durable admission acknowledgement assigns input_seq.
				// Unadmitted reservations and inputs newer than this snapshot stay
				// active and cause the task update below to roll back atomically.
				{
					let query_bind_1 = id;
					let query_bind_2 = run_id;
					let query_bind_3 = through_seq;
					crate::database::native::query(
						&reinhardt::query::Query::update()
							.table(reinhardt::query::Alias::new("remote_run_message_fences"))
							.value(reinhardt::query::Alias::new("consumed"), true)
							.and_where(SimpleExpr::CustomWithExpr(
								"(task_id = ? AND run_id = ? AND input_seq <= ?)".to_owned(),
								vec![
									Expr::value(query_bind_1.to_owned()).into(),
									Expr::value(query_bind_2.to_owned()).into(),
									Expr::value(query_bind_3.to_owned()).into(),
								],
							))
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.execute(&mut **tx)
					.await?
				};
			}
		}
		if task.status == next {
			return Ok(task);
		}
		// Any concurrently reserved key that was not admitted by this executor is
		// intentionally left active; gate_remote_task_terminal then rejects this
		// update instead of losing the correction.
		let updated: Task = { let query_bind_1 = id; let query_bind_2 = revision; let query_bind_3 = owner; let query_bind_4 = next; aidash_server::database::query_as(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("tasks")).value_expr(reinhardt::query::Alias::new("status"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.as_str()).into()])).value_expr(reinhardt::query::Alias::new("owner"), SimpleExpr::CustomWithExpr("(CASE WHEN ? = 'OPEN' THEN NULL ELSE ? END)".to_owned(), vec![Expr::value(query_bind_4.as_str()).into(), Expr::value(query_bind_3.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1"))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND revision = ? AND (owner = ? OR (owner IS NULL AND status = 'OPEN' AND ? IN ('CANCELLED', 'FAILED'))))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.as_str()).into()]))
				.returning_all()
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.fetch_optional(&mut **tx)
		.await? }
		.ok_or_else(|| Error::Conflict("task revision changed".into()))?;
		self.event(
			tx,
			Some(updated.workspace_id),
			"task.updated",
			json!(updated),
		)
		.await?;
		Ok(updated)
	}
	pub(crate) async fn run_message_delivery_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		delivery: RunMessageDelivery<'_>,
	) -> Result<Message> {
		let RunMessageDelivery {
			workspace,
			task_id,
			run_id,
			sender,
			content,
			input_key,
			message_key,
		} = delivery;
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		let reservation: Option<(Uuid, String, bool, bool, bool)> = {
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = input_key;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("run_id"))
					.column(reinhardt::query::Alias::new("content"))
					.expr_as(
						reinhardt::query::Expr::cust("expires_at IS NULL"),
						reinhardt::query::Alias::new("permanent"),
					)
					.expr_as(
						reinhardt::query::Expr::cust(
							"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
						),
						reinhardt::query::Alias::new("unexpired"),
					)
					.column(reinhardt::query::Alias::new("consumed"))
					.from(reinhardt::query::Alias::new("remote_run_message_fences"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.columns(&["run_id", "content", "permanent", "unexpired", "consumed"])
			.fetch_optional(&mut **tx)
			.await?
		};
		let Some((reserved_run, reserved_content, committed, active, consumed)) = reservation
		else {
			return Err(Error::Conflict(
				"remote run message was not reserved at home".into(),
			));
		};
		if reserved_run != run_id || reserved_content != content {
			return Err(Error::Conflict(
				"remote run message reservation changed".into(),
			));
		}
		if !active && !consumed {
			return Err(Error::Conflict(
				"remote run message reservation expired".into(),
			));
		}
		if !committed
			&& !consumed
			&& matches!(
				task.status,
				crate::domain::TaskStatus::Completed
					| crate::domain::TaskStatus::Failed
					| crate::domain::TaskStatus::Cancelled
					| crate::domain::TaskStatus::Abandoned
			) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		// The database gate rejects the old home-write path during rolling
		// upgrades. Only the ledger-backed delivery endpoint sets this marker.
		crate::database::native::query_scalar::<String>(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust(
					"set_config('aidash.run_message_delivery', 'true', true)",
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?;
		let message = self
			.message_in(tx, workspace, sender, content, Some(message_key))
			.await?;
		{
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = input_key;
			let query_bind_4 = content;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("remote_run_message_fences"))
					.value_expr(
						reinhardt::query::Alias::new("expires_at"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND run_id = ? AND idempotency_key = ? AND content = ?)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
							Expr::value(query_bind_4.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
		Ok(message)
	}
	pub(crate) async fn reserve_remote_run_message_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		key: &str,
		content: &str,
	) -> Result<()> {
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		let terminal = matches!(
			task.status,
			crate::domain::TaskStatus::Completed
				| crate::domain::TaskStatus::Failed
				| crate::domain::TaskStatus::Cancelled
				| crate::domain::TaskStatus::Abandoned
		);
		let full_message_key = format!("{peer_node}:{task_id}:{key}");
		let message_exists: bool = {
			let query_bind_1 = task.workspace_id;
			let query_bind_2 = &full_message_key;
			let query_bind_3 = format!("human@{peer_node}");
			let query_bind_4 = content;
			crate::database::native::query_scalar(&reinhardt::query::Query::select()
				.expr(SimpleExpr::CustomWithExpr("(EXISTS(SELECT 1 FROM messages WHERE workspace_id = ? AND idempotency_key = ? AND sender = ? AND content = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.scalar_one(&mut **tx)
		.await?
		};
		let previous: Option<(Uuid, String, bool, bool, bool)> = {
			let query_bind_1 = task_id;
			let query_bind_2 = key;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("run_id"))
					.column(reinhardt::query::Alias::new("content"))
					.expr_as(
						reinhardt::query::Expr::cust("expires_at IS NULL"),
						reinhardt::query::Alias::new("permanent"),
					)
					.expr_as(
						reinhardt::query::Expr::cust(
							"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
						),
						reinhardt::query::Alias::new("unexpired"),
					)
					.column(reinhardt::query::Alias::new("consumed"))
					.from(reinhardt::query::Alias::new("remote_run_message_fences"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND idempotency_key = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.columns(&["run_id", "content", "permanent", "unexpired", "consumed"])
			.fetch_optional(&mut **tx)
			.await?
		};
		if let Some((previous_run, previous_content, committed, active, consumed)) = previous {
			if previous_run != run_id || previous_content != content {
				return Err(Error::Conflict("run message idempotency key reused".into()));
			}
			if terminal {
				if committed || consumed {
					return Ok(());
				}
				if !message_exists {
					return Err(Error::Conflict("home task is terminal".into()));
				}
				{
					let query_bind_1 = task_id;
					let query_bind_2 = run_id;
					let query_bind_3 = key;
					crate::database::native::query(
						&reinhardt::query::Query::update()
							.table(reinhardt::query::Alias::new("remote_run_message_fences"))
							.value_expr(
								reinhardt::query::Alias::new("expires_at"),
								reinhardt::query::Expr::cust("NULL"),
							)
							.and_where(SimpleExpr::CustomWithExpr(
								"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
								vec![
									Expr::value(query_bind_1.to_owned()).into(),
									Expr::value(query_bind_2.to_owned()).into(),
									Expr::value(query_bind_3.to_owned()).into(),
								],
							))
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.execute(&mut **tx)
					.await?
				};
				return Ok(());
			}
			if !committed && !active && !terminal {
				{
					let query_bind_1 = task_id;
					let query_bind_2 = run_id;
					let query_bind_3 = key;
					crate::database::native::query(
						&reinhardt::query::Query::update()
							.table(reinhardt::query::Alias::new("remote_run_message_fences"))
							.value_expr(
								reinhardt::query::Alias::new("expires_at"),
								reinhardt::query::Expr::cust(
									"CURRENT_TIMESTAMP + INTERVAL '60 seconds'",
								),
							)
							.and_where(SimpleExpr::CustomWithExpr(
								"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
								vec![
									Expr::value(query_bind_1.to_owned()).into(),
									Expr::value(query_bind_2.to_owned()).into(),
									Expr::value(query_bind_3.to_owned()).into(),
								],
							))
							.to_string(reinhardt::query::PostgresQueryBuilder),
					)
					.execute(&mut **tx)
					.await?
				};
			}
		} else {
			if terminal && !message_exists {
				return Err(Error::Conflict("home task is terminal".into()));
			}
			{
				let query_bind_1 = task_id;
				let query_bind_2 = run_id;
				let query_bind_3 = key;
				let query_bind_4 = content;
				crate::database::native::query(
					&reinhardt::query::Query::insert()
						.into_table(reinhardt::query::Alias::new("remote_run_message_fences"))
						.columns([
							reinhardt::query::Alias::new("task_id"),
							reinhardt::query::Alias::new("run_id"),
							reinhardt::query::Alias::new("idempotency_key"),
							reinhardt::query::Alias::new("content"),
							reinhardt::query::Alias::new("expires_at"),
						])
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
								.expr(if terminal {
									reinhardt::query::Expr::cust("NULL")
								} else {
									reinhardt::query::Expr::cust(
										"CURRENT_TIMESTAMP + INTERVAL '60 seconds'",
									)
								})
								.to_owned(),
						)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.execute(&mut **tx)
				.await?
			};
		}
		Ok(())
	}
	pub(crate) async fn commit_remote_run_message_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task_id: Uuid,
		run_id: Uuid,
		key: &str,
		content: &str,
		input_seq: Option<i64>,
	) -> Result<()> {
		if input_seq.is_some_and(|seq| seq <= 0) {
			return Err(Error::Invalid("invalid admitted input sequence".into()));
		}
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		let reservation: Option<(String, bool, bool, Option<i64>)> = {
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = key;
			crate::database::native::query_as(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("content"))
					.expr_as(
						reinhardt::query::Expr::cust("expires_at IS NULL"),
						reinhardt::query::Alias::new("permanent"),
					)
					.expr_as(
						reinhardt::query::Expr::cust(
							"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
						),
						reinhardt::query::Alias::new("unexpired"),
					)
					.column(reinhardt::query::Alias::new("input_seq"))
					.from(reinhardt::query::Alias::new("remote_run_message_fences"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.columns(&["content", "permanent", "unexpired", "input_seq"])
			.fetch_optional(&mut **tx)
			.await?
		};
		let Some((reserved_content, committed, active, previous_seq)) = reservation else {
			return Err(Error::Conflict(
				"remote run message was not reserved at home".into(),
			));
		};
		if reserved_content != content {
			return Err(Error::Conflict(
				"remote run message reservation changed".into(),
			));
		}
		if input_seq
			.zip(previous_seq)
			.is_some_and(|(seq, previous)| seq != previous)
		{
			return Err(Error::Conflict("run message input sequence changed".into()));
		}
		if committed && (input_seq.is_none() || input_seq == previous_seq) {
			return Ok(());
		}
		// A run-bound acknowledgement from the delegated executor describes an
		// already persisted input, not a new admission. Its exact fence remains
		// recoverable after expiry or task termination; this never reopens the task.
		if !committed
			&& input_seq.is_none()
			&& (!active
				|| matches!(
					task.status,
					crate::domain::TaskStatus::Completed
						| crate::domain::TaskStatus::Failed
						| crate::domain::TaskStatus::Cancelled
						| crate::domain::TaskStatus::Abandoned
				)) {
			return Err(Error::Conflict(
				"remote run message reservation expired before admission was committed".into(),
			));
		}
		{
			let query_bind_1 = task_id;
			let query_bind_2 = run_id;
			let query_bind_3 = key;
			let query_bind_4 = content;
			let query_bind_5 = input_seq;
			crate::database::native::query(
				&reinhardt::query::Query::update()
					.table(reinhardt::query::Alias::new("remote_run_message_fences"))
					.value_expr(
						reinhardt::query::Alias::new("expires_at"),
						reinhardt::query::Expr::cust("NULL"),
					)
					.value_expr(
						reinhardt::query::Alias::new("input_seq"),
						SimpleExpr::CustomWithExpr(
							"(COALESCE(input_seq, ?))".to_owned(),
							vec![Expr::value(query_bind_5.to_owned()).into()],
						),
					)
					.and_where(SimpleExpr::CustomWithExpr(
						"(task_id = ? AND run_id = ? AND idempotency_key = ? AND content = ?)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
							Expr::value(query_bind_4.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.execute(&mut **tx)
			.await?
		};
		Ok(())
	}
	pub(crate) async fn release_remote_run_message_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		keys: &[String],
	) -> Result<()> {
		// Deletion and task termination must use the same lock order.
		let task: Task = {
			let query_bind_1 = task_id;
			aidash_server::database::query_as(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::col(
						reinhardt::query::ColumnRef::Asterisk,
					))
					.from(reinhardt::query::Alias::new("tasks"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(&mut **tx)
			.await?
		};
		for key in keys {
			let full_key = format!("{peer_node}:{task_id}:{key}");
			{
				let query_bind_1 = task_id;
				let query_bind_2 = run_id;
				let query_bind_3 = key;
				let query_bind_4 = task.workspace_id;
				let query_bind_5 = full_key;
				crate::database::native::query(&reinhardt::query::Query::delete()
					.from_table(reinhardt::query::Alias::new("remote_run_message_fences"))
					.and_where(SimpleExpr::CustomWithExpr("(task_id = ? AND run_id = ? AND idempotency_key = ? AND NOT consumed AND (expires_at IS NOT NULL OR input_seq IS NULL) AND NOT EXISTS (SELECT 1 FROM messages WHERE workspace_id = ? AND idempotency_key = ?))".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_5.to_owned()).into()]))
					.to_string(reinhardt::query::PostgresQueryBuilder))
			.execute(&mut **tx)
			.await?
			};
		}
		Ok(())
	}
	pub(crate) async fn run_message_has_media(&self, message_ids: &[Uuid]) -> Result<bool> {
		if message_ids.is_empty() {
			return Ok(false);
		}
		let id: Option<Uuid> = {
			let query_bind_1 = message_ids;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("id"))
					.from(reinhardt::query::Alias::new("channel_attachments"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(message_id = ANY(?))".into(),
						vec![
							Expr::cust_with_values(
								"CAST(? AS uuid[])",
								[SqlValue::Array(
									ArrayType::Uuid,
									Some(Box::new(
										query_bind_1.iter().copied().map(SqlValue::from).collect(),
									)),
								)],
							)
							.into(),
						],
					))
					.limit(1)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_optional(&self.pool)
			.await?
		};
		Ok(id.is_some())
	}
	/// Borrow the caller's native transaction for both targeted and recovery claims.
	pub(crate) async fn lease_run_in(
		tx: &mut dyn TransactionExecutor,
		worker: Uuid,
		seconds: i32,
		run_id: Option<Uuid>,
		node_id: &str,
		cursor: &mut run_state::RecoveryCursor,
	) -> Result<Option<Run>> {
		aidash_application::activation::lease(
			&mut crate::bootstrap::activation_scheduling_scope(tx),
			worker,
			seconds,
			run_id,
			node_id,
			cursor,
		)
		.await
		.map_err(Into::into)
	}
}

pub use crate::apps::execution::serializers::invocations::Invocation;

pub(crate) use crate::apps::federation::remote::models::input_fences::TerminalInputs as TerminalRunMessageInputs;

use reinhardt::query::{ArrayType, Value as SqlValue};

impl Store {
	pub(crate) async fn pause_for_semantic_execution(
		&self,
		run: &Run,
		worker: Uuid,
		reason: crate::semantic::remote::Failure,
	) -> Result<()> {
		self.pause_for_execution_reason(
			run,
			worker,
			&reason.to_string(),
			"run.semantic_blocked",
			Some(reason),
		)
		.await
	}
}

impl Store {
	async fn pause_for_execution_reason(
		&self,
		run: impl Into<RunMetadata>,
		worker: Uuid,
		reason: &str,
		event_kind: &str,
		semantic_reason: Option<crate::semantic::remote::Failure>,
	) -> Result<()> {
		let run = run.into();
		let mut tx = crate::database::native::begin(&self.pool).await?;
		let changed = { let query_bind_1 = run.id; let query_bind_2 = worker; let query_bind_3 = reason; let query_bind_4 = event_kind; let query_bind_5 = serde_json::to_value(semantic_reason)?; crate::database::native::query(&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("runs")).value_expr(reinhardt::query::Alias::new("pending"), SimpleExpr::CustomWithExpr("(CASE WHEN ?='run.semantic_blocked' AND control='ACTIVE' THEN jsonb_set(pending, '{recovery,semantic_reason}', ?::jsonb, true) ELSE pending END)".to_owned(), vec![Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_5.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("control"), reinhardt::query::Expr::cust(
						"CASE WHEN control = 'CANCELLED' THEN control ELSE 'PAUSED' END",
					)).value_expr(reinhardt::query::Alias::new("error"), SimpleExpr::CustomWithExpr("(CASE WHEN control = 'PAUSED' AND error IS DISTINCT FROM 'identity status unavailable' THEN error ELSE ? END)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).value_expr(reinhardt::query::Alias::new("revision"), reinhardt::query::Expr::cust("revision + 1")).value_expr(reinhardt::query::Alias::new("updated_at"), reinhardt::query::Expr::cust("CURRENT_TIMESTAMP")).value_expr(reinhardt::query::Alias::new("lease_owner"), reinhardt::query::Expr::cust("NULL")).value_expr(reinhardt::query::Alias::new("lease_until"), reinhardt::query::Expr::cust("NULL"))
				.and_where(SimpleExpr::CustomWithExpr("(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into()]))
				.to_string(reinhardt::query::PostgresQueryBuilder))
		.execute(&mut *tx)
		.await? }
		.rows_affected();
		if changed == 0 {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		self.event(
			&mut tx,
			(run.home_node == self.node_id).then_some(run.workspace_id),
			event_kind,
			json!({"run_id":run.id,"task_id":run.task_id}),
		)
		.await?;
		tx.commit().await?;
		Ok(())
	}
}
#[path = "store/run_state.rs"]
pub(crate) mod run_state;

impl Store {
	async fn ensure_run_response_current_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run_id: Uuid,
		worker: Uuid,
		included_input_seq: i64,
	) -> Result<()> {
		let valid: Option<Uuid> = {
			let query_bind_1 = run_id;
			let query_bind_2 = worker;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.column(reinhardt::query::Alias::new("id"))
					.from(reinhardt::query::Alias::new("runs"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id = ? AND lease_owner = ? AND lease_until > CURRENT_TIMESTAMP)"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.lock(reinhardt::query::LockType::Update)
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_optional(&mut **tx)
			.await?
		};
		if valid.is_none() {
			return Err(Error::Conflict(
				"worker lease lost before response effect".into(),
			));
		}
		let stale: bool = {
			let query_bind_1 = run_id;
			let query_bind_2 = included_input_seq;
			crate::database::native::query_scalar(
				&reinhardt::query::Query::select()
					.expr(SimpleExpr::CustomWithExpr(
						"(EXISTS(SELECT 1 FROM run_inputs WHERE run_id = ? AND seq > ?))"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
						],
					))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.scalar_one(&mut **tx)
			.await?
		};
		if stale {
			return Err(Error::StaleInference);
		}
		Ok(())
	}
}

/// Dashboard journals expose bounded previews; durable invocation records keep
/// the original values for recovery and execution.
pub(crate) fn invocation_summary(alias: Option<&str>) -> reinhardt::query::SelectStatement {
	use reinhardt::query::{Alias, Expr, Query};
	let mut query = Query::select();
	for column in [
		"idempotency_key",
		"run_id",
		"tool",
		"status",
		"replay_safe",
		"created_at",
	] {
		if let Some(table) = alias {
			query.column((Alias::new(table), Alias::new(column)));
		} else {
			query.column(Alias::new(column));
		}
	}
	for column in ["input", "result"] {
		let name = alias.map_or_else(|| column.to_owned(), |table| format!("{table}.{column}"));
		query.expr_as(Expr::cust(format!("CASE WHEN octet_length({name}::text) > 1024 THEN jsonb_build_object('truncated',true,'preview',left({name}::text,1024)) ELSE {name} END")), Alias::new(column));
	}
	query
}

impl Store {
	async fn human_request_sqlx_in(
		&self,
		tx: &mut crate::database::native::Transaction,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		if !matches!(
			kind,
			"QUESTION" | "APPROVAL_REQUIRED" | "CONFIRMATION" | "INFORMATION_REQUEST"
		) {
			return Err(Error::Invalid("unknown human request kind".into()));
		}
		nonempty(prompt, "human request prompt")?;
		let h: Option<HumanRequest> = {
			let query_bind_1 = Uuid::new_v4();
			let query_bind_2 = run.workspace_id;
			let query_bind_3 = run.id;
			let query_bind_4 = kind;
			let query_bind_5 = prompt;
			let query_bind_6 = key;
			aidash_server::database::query_as(
				&reinhardt::query::Query::insert()
					.into_table(reinhardt::query::Alias::new("human_requests"))
					.columns([
						reinhardt::query::Alias::new("id"),
						reinhardt::query::Alias::new("workspace_id"),
						reinhardt::query::Alias::new("run_id"),
						reinhardt::query::Alias::new("kind"),
						reinhardt::query::Alias::new("prompt"),
						reinhardt::query::Alias::new("request_key"),
					])
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
							.expr(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_6.to_owned()).into()],
							))
							.to_owned(),
					)
					.on_conflict(
						reinhardt::query::OnConflict::columns([reinhardt::query::Alias::new(
							"request_key",
						)])
						.do_nothing()
						.to_owned(),
					)
					.returning_all()
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_optional(&mut **tx)
			.await?
		};
		let h = match h {
			Some(h) => {
				self.event(
					tx,
					(run.home_node == self.node_id).then_some(run.workspace_id),
					"human.requested",
					json!(h),
				)
				.await?;
				h
			}
			None => {
				let query_bind_1 = key;
				aidash_server::database::query_as(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
						))
						.from(reinhardt::query::Alias::new("human_requests"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(request_key = ?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			}
		};
		if h.run_id != run.id || h.kind != kind || h.prompt != prompt {
			return Err(Error::Conflict(
				"human request key reused with different input".into(),
			));
		}
		Ok(h)
	}
}

#[cfg(test)]
#[path = "../tests/targeted_leases.rs"]
mod targeted_leases;
