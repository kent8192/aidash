use crate::{
	Error, Result,
	domain::*,
	registry::{Entry, Search},
};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction, postgres::PgPoolOptions};
use std::future::Future;
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
	pub pool: PgPool,
	pub control_pool: PgPool,
	pub node_id: String,
	pub semantic_client: reqwest::Client,
}

#[derive(sqlx::FromRow)]
pub struct RunInput {
	pub seq: i64,
	pub sender: String,
	pub content: String,
	pub idempotency_key: String,
	pub message_id: Option<Uuid>,
	pub reference_only: bool,
}

pub(crate) struct RunResponseMessage<'a> {
	pub run: &'a Run,
	pub worker: Uuid,
	pub included_input_seq: i64,
	pub sender: &'a str,
	pub content: &'a str,
	pub key: &'a str,
	pub track_output: bool,
}

pub(crate) struct RunMessageDelivery<'a> {
	pub workspace: Uuid,
	pub task_id: Uuid,
	pub run_id: Uuid,
	pub sender: &'a str,
	pub content: &'a str,
	pub input_key: &'a str,
	pub message_key: &'a str,
}

pub(crate) struct FencedRunMessageOutput<'a> {
	pub workspace: Uuid,
	pub task_id: Uuid,
	pub run_id: Uuid,
	pub included_input_seq: i64,
	pub sender: &'a str,
	pub content: &'a str,
	pub key: &'a str,
}

fn run_input_size(sender: &str, content: &str) -> usize {
	// The provider receives JSON records, so count escaped content as well as
	// framing. This is the same conservative byte-based estimate as Context.
	serde_json::to_string(&json!({"seq":i64::MAX,"sender":sender,"content":content}))
		.map_or(usize::MAX, |value| crate::context::estimated_tokens(&value))
}

fn run_input_reference_size(sender: &str, message_id: Uuid) -> usize {
	serde_json::to_string(&json!({"seq":i64::MAX,"sender":sender,"record":{"kind":"message","id":message_id},"requires_workspace_read":true}))
		.map_or(usize::MAX, |value| crate::context::estimated_tokens(&value))
}

fn run_input_context_size(input: &RunInput) -> usize {
	if input.reference_only {
		input
			.message_id
			.map_or(usize::MAX, |id| run_input_reference_size(&input.sender, id))
	} else {
		run_input_size(&input.sender, &input.content)
	}
}

enum TerminalRunMessageInputs<'a> {
	Keys(&'a [String]),
	Through(i64),
}

impl Store {
	async fn ensure_run_response_current_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		run_id: Uuid,
		worker: Uuid,
		included_input_seq: i64,
	) -> Result<()> {
		let valid: Option<Uuid> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.column(sea_orm::sea_query::Alias::new("id"))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP",
				))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.bind(worker)
		.fetch_optional(&mut **tx)
		.await?;
		if valid.is_none() {
			return Err(Error::Conflict(
				"worker lease lost before response effect".into(),
			));
		}
		let stale: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM run_inputs WHERE run_id = $1 AND seq > $2)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.bind(included_input_seq)
		.fetch_one(&mut **tx)
		.await?;
		if stale {
			return Err(Error::StaleInference);
		}
		Ok(())
	}
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
		let mut tx = self.pool.begin().await?;
		self.ensure_run_response_current_in(&mut tx, run_id, worker, included_input_seq)
			.await?;
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
		let mut tx = self.pool.begin().await?;
		self.ensure_run_response_current_in(&mut tx, run.id, worker, included_input_seq)
			.await?;
		sqlx::query_scalar::<_, String>(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"set_config('aidash.input_ledger_worker', 'true', true)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
		let message = self
			.message_in(&mut tx, run.workspace_id, sender, content, Some(key))
			.await?;
		if track_output {
			self.record_output_in(
				&mut tx,
				Some(run.id),
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
		let scoped: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM authorization_workspaces WHERE workspace_id = $1)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(workspace)
		.fetch_one(&self.pool)
		.await?;
		if scoped {
			return Err(Error::Forbidden);
		}
		Ok(())
	}

	pub async fn migrate(pool: &PgPool) -> Result<()> {
		use migration::MigratorTrait;
		// Replicas may start together during a rollout. Serialize the complete
		// migrator, including its initial migration ledger creation.
		let mut lease = pool.begin().await?;
		sqlx::query(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"PG_ADVISORY_XACT_LOCK(71003203)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.execute(&mut *lease)
		.await?;
		let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
		migration::Migrator::up(&db, None).await?;
		lease.commit().await?;
		Ok(())
	}
	pub async fn connect(url: &str, node_id: String) -> Result<Self> {
		let pool = PgPoolOptions::new()
			.max_connections(16)
			.connect(url)
			.await?;
		Self::migrate(&pool).await?;
		Self::from_pool(pool, node_id).await
	}

	pub async fn from_pool(pool: PgPool, node_id: String) -> Result<Self> {
		let control_pool = pool
			.options()
			.clone()
			.max_connections(16)
			.connect_with(pool.connect_options().as_ref().clone())
			.await?;
		Ok(Self {
			pool,
			control_pool,
			node_id,
			semantic_client: crate::semantic::backend::client()?,
		})
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
			.await?;
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
			pool,
			control_pool: self.control_pool.clone(),
			node_id: self.node_id.clone(),
			semantic_client: self.semantic_client.clone(),
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
		let control_pool = self
			.control_pool
			.options()
			.clone()
			.max_connections(4)
			.idle_timeout(std::time::Duration::from_secs(10))
			.connect_with(self.control_pool.connect_options().as_ref().clone())
			.await?;
		Ok(Self {
			pool,
			control_pool,
			node_id: self.node_id.clone(),
			semantic_client: self.semantic_client.clone(),
		})
	}
	pub async fn event(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<Event> {
		// Sequence allocation and commit order must agree for Last-Event-ID replay.
		sqlx::query(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"PG_ADVISORY_XACT_LOCK(71003201)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?;
		Ok(sqlx::query_as(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("events"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("node_id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("kind"),
					sea_orm::sea_query::Alias::new("data"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
				])
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(Uuid::new_v4())
		.bind(&self.node_id)
		.bind(workspace)
		.bind(kind)
		.bind(data)
		.fetch_one(&mut **tx)
		.await?)
	}
	pub async fn emit(&self, workspace: Option<Uuid>, kind: &str, data: Value) -> Result<Event> {
		let mut tx = self.pool.begin().await?;
		let event = self.event(&mut tx, workspace, kind, data).await?;
		tx.commit().await?;
		Ok(event)
	}
	pub async fn create_workspace(&self, title: &str, goal: &str) -> Result<Workspace> {
		let mut tx = self.pool.begin().await?;
		let workspace = self
			.create_workspace_in(&mut tx, Uuid::new_v4(), title, goal)
			.await?;
		tx.commit().await?;
		Ok(workspace)
	}
	pub(crate) async fn create_workspace_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		id: Uuid,
		title: &str,
		goal: &str,
	) -> Result<Workspace> {
		nonempty(title, "title")?;
		nonempty(goal, "goal")?;
		let w: Workspace = sqlx::query_as(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("workspaces"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("title"),
					sea_orm::sea_query::Alias::new("goal"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
				])
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(title)
		.bind(goal)
		.fetch_one(&mut **tx)
		.await?;
		self.event(tx, Some(w.id), "workspace.created", json!(w))
			.await?;
		Ok(w)
	}
	pub async fn workspaces(&self) -> Result<Vec<Workspace>> {
		Ok(sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("workspaces"))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("created_at"),
					)),
					sea_orm::sea_query::Order::Desc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_all(&self.pool)
		.await?)
	}
	pub async fn workspace(&self, id: Uuid) -> Result<Workspace> {
		sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("workspaces"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&self.pool)
		.await?
		.ok_or_else(|| Error::NotFound("workspace".into()))
	}
	pub async fn update_state(&self, id: Uuid, revision: i64, state: Value) -> Result<Workspace> {
		let mut tx = self.pool.begin().await?;
		let workspace = self.update_state_in(&mut tx, id, revision, state).await?;
		tx.commit().await?;
		Ok(workspace)
	}
	pub(crate) async fn update_state_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<Workspace> {
		if !state.is_object() {
			return Err(Error::Invalid("workspace state must be an object".into()));
		}
		let w = sqlx::query_as(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("workspaces"))
				.value(
					sea_orm::sea_query::Alias::new("state"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("id = $1 AND revision = $2"))
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(revision)
		.bind(state)
		.fetch_optional(&mut **tx)
		.await?
		.ok_or_else(|| Error::Conflict("workspace revision changed".into()))?;
		self.event(tx, Some(id), "workspace.updated", json!(w))
			.await?;
		Ok(w)
	}
	pub async fn task(&self, id: Uuid) -> Result<Task> {
		sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&self.pool)
		.await?
		.ok_or_else(|| Error::NotFound("task".into()))
	}
	pub async fn task_page(&self, offset: u64) -> Result<crate::api_schema::TaskPage> {
		use sea_orm::sea_query::{Alias, Asterisk, Order, PostgresQueryBuilder, Query};
		let tasks: Vec<Task> = sqlx::query_as(
			&Query::select()
				.column(Asterisk)
				.from(Alias::new("tasks"))
				.order_by(Alias::new("created_at"), Order::Desc)
				.order_by(Alias::new("id"), Order::Desc)
				.limit(500)
				.offset(offset)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(&self.pool)
		.await?;
		Ok(crate::api_schema::TaskPage {
			next_offset: (tasks.len() == 500).then_some(offset.saturating_add(500)),
			tasks,
		})
	}
	pub async fn tasks(&self, workspace: Option<Uuid>) -> Result<Vec<Task>> {
		Ok(sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"(CAST($1 AS UUID) IS NULL OR workspace_id = $1)",
				))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("created_at"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("id"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(workspace)
		.fetch_all(&self.pool)
		.await?)
	}
	pub async fn create_task(
		&self,
		workspace: Uuid,
		input: &NewTask,
		creator: &str,
		key: Option<&str>,
	) -> Result<Task> {
		let mut tx = self.pool.begin().await?;
		let task = self
			.create_task_in(&mut tx, workspace, input, creator, key)
			.await?;
		tx.commit().await?;
		Ok(task)
	}
	pub(crate) async fn create_task_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
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
			let valid: bool = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust(
						"EXISTS(SELECT 1 FROM tasks WHERE id = $1 AND workspace_id = $2)",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(dep)
			.bind(workspace)
			.fetch_one(&mut **tx)
			.await?;
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
			ancestor = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.column(sea_orm::sea_query::Alias::new("parent_id"))
					.from(sea_orm::sea_query::Alias::new("tasks"))
					.and_where(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id"))
							.eq(sea_orm::sea_query::Expr::cust("$1")),
					)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.fetch_one(&mut **tx)
			.await?;
		}
		if let Some(parent) = input.parent_id {
			// Completion takes the same row lock before testing its children.
			let status: String = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("status")),
					))
					.from(sea_orm::sea_query::Alias::new("tasks"))
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.lock(sea_orm::sea_query::LockType::Update)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(parent)
			.fetch_one(&mut **tx)
			.await?;
			let replay: bool = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust(
						"EXISTS(SELECT 1 FROM tasks WHERE creation_key = $1)",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(key)
			.fetch_one(&mut **tx)
			.await?;
			if matches!(
				status.as_str(),
				"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
			) && !replay
			{
				return Err(Error::Conflict(
					"cannot add a child to a terminal parent".into(),
				));
			}
		}
		let task: Option<Task> = sqlx::query_as(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("tasks"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("title"),
					sea_orm::sea_query::Alias::new("description"),
					sea_orm::sea_query::Alias::new("requirements"),
					sea_orm::sea_query::Alias::new("created_by"),
					sea_orm::sea_query::Alias::new("dependencies"),
					sea_orm::sea_query::Alias::new("parent_id"),
					sea_orm::sea_query::Alias::new("creation_key"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
					sea_orm::sea_query::Expr::cust("$6"),
					sea_orm::sea_query::Expr::cust("$7"),
					sea_orm::sea_query::Expr::cust("$8"),
					sea_orm::sea_query::Expr::cust("$9"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new(
						"creation_key",
					)])
					.do_nothing()
					.to_owned(),
				)
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(Uuid::new_v4())
		.bind(workspace)
		.bind(&input.title)
		.bind(&input.description)
		.bind(&input.requirements)
		.bind(creator)
		.bind(&input.dependencies)
		.bind(input.parent_id)
		.bind(key)
		.fetch_optional(&mut **tx)
		.await?;
		let task = match task {
			Some(t) => {
				self.event(tx, Some(workspace), "task.created", json!(t))
					.await?;
				t
			}
			None => {
				let t: Task = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("tasks"))
						.and_where(sea_orm::sea_query::Expr::cust("creation_key = $1"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(key)
				.fetch_one(&mut **tx)
				.await?;
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
		let mut tx = self.pool.begin().await?;
		let claimed = self
			.claim_in(&mut tx, &task, revision, owner, agent)
			.await?;
		tx.commit().await?;
		Ok(claimed)
	}
	pub(crate) async fn claim_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		task: &Task,
		revision: i64,
		owner: &str,
		agent: &Entry,
	) -> Result<Task> {
		let id = task.id;
		let mut requirements: Search = serde_json::from_value(task.requirements.clone())?;
		requirements.kind = Some("agent".into());
		if !requirements.matches(agent) {
			return Err(Error::Invalid(
				"agent does not satisfy task requirements".into(),
			));
		}
		let claimed: Task = sqlx::query_as(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("tasks")).value(sea_orm::sea_query::Alias::new("status"), sea_orm::sea_query::Expr::cust("'CLAIMED'")).value(sea_orm::sea_query::Alias::new("owner"), sea_orm::sea_query::Expr::cust("$3")).value(sea_orm::sea_query::Alias::new("revision"), sea_orm::sea_query::Expr::cust("revision + 1")).and_where(sea_orm::sea_query::Expr::cust("id = $1 AND revision = $2 AND status = 'OPEN' AND NOT EXISTS(SELECT 1 FROM delegations AS d WHERE d.task_id = tasks.id AND d.node_id || '/agents/' || d.agent_id || '@' || d.agent_version <> $3) AND NOT EXISTS(SELECT 1 FROM tasks AS d WHERE d.id = ANY(tasks.dependencies) AND d.status <> 'COMPLETED')")).returning_all().to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(id).bind(revision).bind(owner).fetch_optional(&mut **tx).await?.ok_or_else(|| Error::Conflict("task already claimed, revision changed, or dependencies are incomplete".into()))?;
		if owner == qualified_agent(&self.node_id, &agent.id, &agent.version) {
			// Persist the local execution with the claim; no crash can strand a
			// claimed task between the control API and its worker queue.
			sqlx::query(
				&sea_orm::sea_query::Query::insert()
					.into_table(sea_orm::sea_query::Alias::new("runs"))
					.columns([
						sea_orm::sea_query::Alias::new("id"),
						sea_orm::sea_query::Alias::new("task_id"),
						sea_orm::sea_query::Alias::new("workspace_id"),
						sea_orm::sea_query::Alias::new("home_node"),
						sea_orm::sea_query::Alias::new("agent_id"),
						sea_orm::sea_query::Alias::new("agent_version"),
					])
					.values_panic([
						sea_orm::sea_query::Expr::cust("$1"),
						sea_orm::sea_query::Expr::cust("$2"),
						sea_orm::sea_query::Expr::cust("$3"),
						sea_orm::sea_query::Expr::cust("$4"),
						sea_orm::sea_query::Expr::cust("$5"),
						sea_orm::sea_query::Expr::cust("$6"),
					])
					.on_conflict(
						sea_orm::sea_query::OnConflict::columns([
							sea_orm::sea_query::Alias::new("home_node"),
							sea_orm::sea_query::Alias::new("task_id"),
						])
						.do_nothing()
						.to_owned(),
					)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(Uuid::new_v4())
			.bind(id)
			.bind(claimed.workspace_id)
			.bind(&self.node_id)
			.bind(&agent.id)
			.bind(&agent.version)
			.execute(&mut **tx)
			.await?;
			let executor: (String, String) = sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("agent_id")),
					))
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
							"agent_version",
						)),
					))
					.from(sea_orm::sea_query::Alias::new("runs"))
					.and_where(sea_orm::sea_query::Expr::cust(
						"home_node = $1 AND task_id = $2",
					))
					.lock(sea_orm::sea_query::LockType::Update)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(&self.node_id)
			.bind(id)
			.fetch_one(&mut **tx)
			.await?;
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
		next: &str,
	) -> Result<Task> {
		let task = self.task(id).await?;
		let terminate_unclaimed =
			matches!(next, "CANCELLED" | "FAILED") && task.status == "OPEN" && task.owner.is_none();
		if task.owner.as_deref() != Some(owner) && !terminate_unclaimed {
			return Err(Error::Unauthorized);
		}
		let before: TaskStatus = serde_json::from_value(json!(task.status))?;
		let after: TaskStatus = serde_json::from_value(json!(next))
			.map_err(|_| Error::Invalid("invalid task status".into()))?;
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
		let mut tx = self.pool.begin().await?;
		let t: Task = sqlx::query_as(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("tasks")).value(sea_orm::sea_query::Alias::new("status"), sea_orm::sea_query::Expr::cust("$4")).value(sea_orm::sea_query::Alias::new("owner"), sea_orm::sea_query::Expr::cust("CASE WHEN $4 = 'OPEN' THEN NULL ELSE $3 END")).value(sea_orm::sea_query::Alias::new("revision"), sea_orm::sea_query::Expr::cust("revision + 1")).and_where(sea_orm::sea_query::Expr::cust("id = $1 AND revision = $2 AND (owner = $3 OR (owner IS NULL AND status = 'OPEN' AND $4 IN ('CANCELLED', 'FAILED')))")).returning_all().to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(id).bind(revision).bind(owner).bind(next).fetch_optional(&mut *tx).await?.ok_or_else(|| Error::Conflict("task revision changed".into()))?;
		self.event(&mut tx, Some(t.workspace_id), "task.updated", json!(t))
			.await?;
		tx.commit().await?;
		Ok(t)
	}
	/// Consume this remote run's admitted-message fences and perform an explicit
	/// cancellation or failure while holding the same authoritative task lock.
	pub async fn transition_remote_run_message_terminal(
		&self,
		id: Uuid,
		revision: i64,
		owner: &str,
		next: &str,
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
		next: &str,
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
		next: &str,
		run_id: Uuid,
		inputs: TerminalRunMessageInputs<'_>,
	) -> Result<Task> {
		if !matches!(next, "CANCELLED" | "FAILED") {
			return Err(Error::Invalid(
				"remote run-message terminal transition must be cancelled or failed".into(),
			));
		}
		let mut tx = self.pool.begin().await?;
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_one(&mut *tx)
		.await?;
		let terminate_unclaimed =
			task.status == "OPEN" && task.owner.is_none() && matches!(next, "CANCELLED" | "FAILED");
		if task.owner.as_deref() != Some(owner) && !terminate_unclaimed {
			return Err(Error::Unauthorized);
		}
		if task.revision != revision {
			return Err(Error::Conflict("task revision changed".into()));
		}
		if task.status != next {
			let before: TaskStatus = serde_json::from_value(json!(task.status))?;
			let after: TaskStatus = serde_json::from_value(json!(next))
				.map_err(|_| Error::Invalid("invalid task status".into()))?;
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
					sqlx::query(
						&sea_orm::sea_query::Query::update()
							.table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
							.value(
								sea_orm::sea_query::Alias::new("consumed"),
								sea_orm::sea_query::Expr::cust("TRUE"),
							)
							.and_where(sea_orm::sea_query::Expr::cust(
								"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
							))
							.to_string(sea_orm::sea_query::PostgresQueryBuilder),
					)
					.bind(id)
					.bind(run_id)
					.bind(key)
					.execute(&mut *tx)
					.await?;
				}
			}
			TerminalRunMessageInputs::Through(through_seq) => {
				// Only a durable admission acknowledgement assigns input_seq.
				// Unadmitted reservations and inputs newer than this snapshot stay
				// active and cause the task update below to roll back atomically.
				sqlx::query(
					&sea_orm::sea_query::Query::update()
						.table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
						.value(sea_orm::sea_query::Alias::new("consumed"), true)
						.and_where(sea_orm::sea_query::Expr::cust(
							"task_id = $1 AND run_id = $2 AND input_seq <= $3",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(id)
				.bind(run_id)
				.bind(through_seq)
				.execute(&mut *tx)
				.await?;
			}
		}
		if task.status == next {
			tx.commit().await?;
			return Ok(task);
		}
		// Any concurrently reserved key that was not admitted by this executor is
		// intentionally left active; gate_remote_task_terminal then rejects this
		// update instead of losing the correction.
		let updated: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("tasks"))
				.value(
					sea_orm::sea_query::Alias::new("status"),
					sea_orm::sea_query::Expr::cust("$4"),
				)
				.value(
					sea_orm::sea_query::Alias::new("owner"),
					sea_orm::sea_query::Expr::cust(
						"CASE WHEN $4 = 'OPEN' THEN NULL ELSE $3 END",
					),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND revision = $2 AND (owner = $3 OR (owner IS NULL AND status = 'OPEN' AND $4 IN ('CANCELLED', 'FAILED')))",
				))
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(revision)
		.bind(owner)
		.bind(next)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or_else(|| Error::Conflict("task revision changed".into()))?;
		self.event(
			&mut tx,
			Some(updated.workspace_id),
			"task.updated",
			json!(updated),
		)
		.await?;
		tx.commit().await?;
		Ok(updated)
	}
	/// Explicit operator abandonment preserves the failed outcome and reason
	/// while allowing the parent to finish using the remaining results.
	pub async fn abandon_task(&self, id: Uuid, revision: i64, reason: &str) -> Result<Task> {
		let mut tx = self.pool.begin().await?;
		let task = self
			.abandon_task_in(&mut tx, id, revision, reason, "human")
			.await?;
		tx.commit().await?;
		Ok(task)
	}
	pub(crate) async fn abandon_task_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		id: Uuid,
		revision: i64,
		reason: &str,
		actor: &str,
	) -> Result<Task> {
		nonempty(reason, "abandonment reason")?;
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_one(&mut **tx)
		.await?;
		if task.revision != revision
			|| !matches!(task.status.as_str(), "FAILED" | "BLOCKED" | "CANCELLED")
		{
			return Err(Error::Conflict(
				"only a failed, blocked or cancelled task at the current revision can be abandoned"
					.into(),
			));
		}
		let active_children: bool = sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM tasks WHERE parent_id = $1 AND NOT status IN ('COMPLETED', 'ABANDONED'))")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(id).fetch_one(&mut **tx).await?;
		if active_children {
			return Err(Error::Conflict(
				"resolve or abandon this task's children first".into(),
			));
		}
		let updated: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("tasks"))
				.value(
					sea_orm::sea_query::Alias::new("status"),
					sea_orm::sea_query::Expr::cust("'ABANDONED'"),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_one(&mut **tx)
		.await?;
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
		nonempty(key, "idempotency key")?;
		artifact.validate()?;
		let mut tx = self.pool.begin().await?;
		let t: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_one(&mut *tx)
		.await?;
		if t.owner.as_deref() != Some(owner) {
			return Err(Error::Unauthorized);
		}
		let existing: Option<Artifact> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("artifacts"))
				.and_where(sea_orm::sea_query::Expr::cust("idempotency_key = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(key)
		.fetch_optional(&mut *tx)
		.await?;
		if let Some(a) = existing {
			if a.task_id != id
				|| a.created_by != owner
				|| a.content != artifact.content
				|| a.kind != artifact.kind
				|| a.name != artifact.name
				|| t.status != "COMPLETED"
			{
				return Err(Error::Conflict(
					"completion key reused with different input".into(),
				));
			}
			self.record_output_in(&mut tx, source_run, t.workspace_id, "artifact", a.id)
				.await?;
			tx.commit().await?;
			return Ok(t);
		}
		if t.status != "RUNNING" {
			return Err(Error::Conflict("only a running task can complete".into()));
		}
		let unresolved: bool = sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM tasks WHERE parent_id = $1 AND NOT status IN ('COMPLETED', 'ABANDONED'))")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(id).fetch_one(&mut *tx).await?;
		if unresolved {
			return Err(Error::Conflict("task has unresolved children".into()));
		}
		if let Some((run_id, through_seq)) = remote_run_fence {
			let unobserved: bool = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust(
						"EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND NOT consumed AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP) AND (run_id <> $2 OR input_seq IS NULL OR input_seq > $3))",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.bind(run_id)
			.bind(through_seq)
			.fetch_one(&mut *tx)
			.await?;
			if unobserved {
				return Err(Error::TransactionPending);
			}
			// Keep these fences through every corrected output write. Consume them
			// atomically with terminal completion so an old worker cannot publish
			// between acknowledgement and task completion.
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
					.value(
						sea_orm::sea_query::Alias::new("consumed"),
						sea_orm::sea_query::Expr::cust("TRUE"),
					)
					.and_where(sea_orm::sea_query::Expr::cust(
						"task_id = $1 AND run_id = $2 AND NOT consumed AND input_seq <= $3",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.bind(run_id)
			.bind(through_seq)
			.execute(&mut *tx)
			.await?;
		}
		let a: Artifact = sqlx::query_as(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("artifacts"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("task_id"),
					sea_orm::sea_query::Alias::new("kind"),
					sea_orm::sea_query::Alias::new("name"),
					sea_orm::sea_query::Alias::new("content"),
					sea_orm::sea_query::Alias::new("created_by"),
					sea_orm::sea_query::Alias::new("idempotency_key"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
					sea_orm::sea_query::Expr::cust("$6"),
					sea_orm::sea_query::Expr::cust("$7"),
					sea_orm::sea_query::Expr::cust("$8"),
				])
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(Uuid::new_v4())
		.bind(t.workspace_id)
		.bind(id)
		.bind(&artifact.kind)
		.bind(&artifact.name)
		.bind(&artifact.content)
		.bind(owner)
		.bind(key)
		.fetch_one(&mut *tx)
		.await?;
		self.record_output_in(&mut tx, source_run, t.workspace_id, "artifact", a.id)
			.await?;
		let t: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("tasks"))
				.value(
					sea_orm::sea_query::Alias::new("status"),
					sea_orm::sea_query::Expr::cust("'COMPLETED'"),
				)
				.value(
					sea_orm::sea_query::Alias::new("completion_key"),
					sea_orm::sea_query::Expr::cust("$2"),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(key)
		.fetch_one(&mut *tx)
		.await?;
		self.event(
			&mut tx,
			Some(t.workspace_id),
			"task.completed",
			json!({"task":t,"artifact":a}),
		)
		.await?;
		tx.commit().await?;
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
		input.validate()?;
		let mut tx = self.pool.begin().await?;
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut *tx)
		.await?;
		if task.owner.as_deref() != Some(owner) {
			return Err(Error::Unauthorized);
		}
		let a: Option<Artifact> = sqlx::query_as(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("artifacts"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("task_id"),
					sea_orm::sea_query::Alias::new("kind"),
					sea_orm::sea_query::Alias::new("name"),
					sea_orm::sea_query::Alias::new("content"),
					sea_orm::sea_query::Alias::new("created_by"),
					sea_orm::sea_query::Alias::new("idempotency_key"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
					sea_orm::sea_query::Expr::cust("$6"),
					sea_orm::sea_query::Expr::cust("$7"),
					sea_orm::sea_query::Expr::cust("$8"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new(
						"idempotency_key",
					)])
					.do_nothing()
					.to_owned(),
				)
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(Uuid::new_v4())
		.bind(task.workspace_id)
		.bind(task_id)
		.bind(&input.kind)
		.bind(&input.name)
		.bind(&input.content)
		.bind(owner)
		.bind(key)
		.fetch_optional(&mut *tx)
		.await?;
		let a = match a {
			Some(a) => {
				self.event(
					&mut tx,
					Some(task.workspace_id),
					"artifact.published",
					json!(a),
				)
				.await?;
				a
			}
			None => {
				let a: Artifact = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("artifacts"))
						.and_where(sea_orm::sea_query::Expr::cust("idempotency_key = $1"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(key)
				.fetch_one(&mut *tx)
				.await?;
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
		self.record_output_in(&mut tx, source_run, task.workspace_id, "artifact", a.id)
			.await?;
		tx.commit().await?;
		Ok(a)
	}
	pub async fn events(
		&self,
		after: i64,
		workspace: Option<Uuid>,
		limit: i64,
	) -> Result<Vec<Event>> {
		Ok(sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("sequence")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("workspace_id")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("kind")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("data")),
				))
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("created_at")),
				))
				.from(sea_orm::sea_query::Alias::new("events"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"sequence > $1 AND (CAST($2 AS UUID) IS NULL OR workspace_id = $2)",
				))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("sequence"),
					)),
					sea_orm::sea_query::Order::Asc,
				)
				.limit(limit.clamp(1, 1000) as u64)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(after.max(0))
		.bind(workspace)
		.fetch_all(&self.pool)
		.await?)
	}
	pub async fn snapshot_page(
		&self,
		workspace: Uuid,
		collection: &str,
		after: Option<Uuid>,
	) -> Result<SnapshotPage> {
		use sea_orm::sea_query::{Alias, Asterisk, Expr, Order, PostgresQueryBuilder, Query};
		if !matches!(collection, "tasks" | "artifacts" | "events" | "messages") {
			return Err(Error::Invalid("unknown snapshot collection".into()));
		}
		let mut source = Query::select();
		source
			.column(Asterisk)
			.from(Alias::new(collection))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")));
		if collection == "events" {
			source
				.order_by(Alias::new("sequence"), Order::Desc)
				.limit(100);
		}
		if collection == "messages" {
			source
				.order_by(Alias::new("created_at"), Order::Desc)
				.order_by(Alias::new("id"), Order::Desc)
				.limit(100);
		}
		let rows: Vec<(Uuid, Value)> = sqlx::query_as(
			&Query::select()
				.column((Alias::new("item"), Alias::new("id")))
				.expr(Expr::cust("to_jsonb(item)"))
				.from_subquery(source, Alias::new("item"))
				.and_where(Expr::cust("$2::uuid IS NULL OR item.id>$2"))
				.order_by((Alias::new("item"), Alias::new("id")), Order::Asc)
				.limit(32)
				.to_string(PostgresQueryBuilder),
		)
		.bind(workspace)
		.bind(after)
		.fetch_all(&self.pool)
		.await?;
		let full = rows.len() == 32;
		let mut page = SnapshotPage {
			items: vec![],
			next: None,
		};
		let mut size = 128;
		let mut last = after;
		for (id, item) in rows {
			let item_size = serde_json::to_vec(&item)?.len() + 1;
			if size + item_size > 3_145_728 {
				if page.items.is_empty() {
					return Err(Error::Invalid(
						"individual workspace resource exceeds the 3 MiB federation page limit"
							.into(),
					));
				}
				page.next = last;
				return Ok(page);
			}
			size += item_size;
			page.items.push(item);
			last = Some(id);
		}
		if full {
			page.next = last;
		}
		Ok(page)
	}
	/// Read one exact workspace record without relying on the bounded recent
	/// event/message collections in `snapshot`.
	pub async fn workspace_record(&self, workspace: Uuid, kind: &str, id: Uuid) -> Result<Value> {
		match kind {
			"workspace" if workspace == id => Ok(json!(self.workspace(workspace).await?)),
			"workspace" => Err(Error::Invalid("workspace record not available".into())),
			"task" => self.workspace_record_row("tasks", workspace, id).await,
			"artifact" => self.workspace_record_row("artifacts", workspace, id).await,
			"message" => self.workspace_record_row("messages", workspace, id).await,
			"event" => self.workspace_record_row("events", workspace, id).await,
			_ => Err(Error::Invalid("unknown workspace record kind".into())),
		}
	}

	async fn workspace_record_row(&self, table: &str, workspace: Uuid, id: Uuid) -> Result<Value> {
		use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
		let record = match table {
			"tasks" => {
				let query = Query::select()
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("title"),
						Alias::new("description"),
						Alias::new("status"),
						Alias::new("requirements"),
						Alias::new("owner"),
						Alias::new("created_by"),
						Alias::new("dependencies"),
						Alias::new("parent_id"),
						Alias::new("revision"),
						Alias::new("created_at"),
					])
					.from(Alias::new("tasks"))
					.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
					.to_string(PostgresQueryBuilder);
				sqlx::query_as::<_, Task>(&query)
					.bind(workspace)
					.bind(id)
					.fetch_optional(&self.pool)
					.await?
					.map(|record| json!(record))
			}
			"artifacts" => {
				let query = Query::select()
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("task_id"),
						Alias::new("kind"),
						Alias::new("name"),
						Alias::new("content"),
						Alias::new("created_by"),
						Alias::new("idempotency_key"),
						Alias::new("created_at"),
					])
					.from(Alias::new("artifacts"))
					.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
					.to_string(PostgresQueryBuilder);
				sqlx::query_as::<_, Artifact>(&query)
					.bind(workspace)
					.bind(id)
					.fetch_optional(&self.pool)
					.await?
					.map(|record| json!(record))
			}
			"messages" => {
				let query = Query::select()
					.columns([
						Alias::new("id"),
						Alias::new("workspace_id"),
						Alias::new("sender"),
						Alias::new("content"),
						Alias::new("idempotency_key"),
						Alias::new("created_at"),
					])
					.from(Alias::new("messages"))
					.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
					.to_string(PostgresQueryBuilder);
				sqlx::query_as::<_, Message>(&query)
					.bind(workspace)
					.bind(id)
					.fetch_optional(&self.pool)
					.await?
					.map(|record| json!(record))
			}
			"events" => {
				let query = Query::select()
					.columns([
						Alias::new("sequence"),
						Alias::new("id"),
						Alias::new("node_id"),
						Alias::new("workspace_id"),
						Alias::new("kind"),
						Alias::new("data"),
						Alias::new("created_at"),
					])
					.from(Alias::new("events"))
					.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$2")))
					.to_string(PostgresQueryBuilder);
				sqlx::query_as::<_, Event>(&query)
					.bind(workspace)
					.bind(id)
					.fetch_optional(&self.pool)
					.await?
					.map(|record| json!(record))
			}
			_ => return Err(Error::Invalid("unknown workspace record kind".into())),
		};
		record.ok_or_else(|| Error::Invalid("workspace record not available".into()))
	}

	pub async fn child_task_summary(
		&self,
		workspace: Uuid,
		parent: Uuid,
	) -> Result<ChildTaskSummary> {
		use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
		let query = Query::select()
			.expr(Expr::cust(
				"COALESCE(BOOL_OR(status NOT IN ('COMPLETED', 'ABANDONED')), FALSE)",
			))
			.expr(Expr::cust(
				"COALESCE(BOOL_OR(status IN ('FAILED', 'BLOCKED', 'CANCELLED')), FALSE)",
			))
			.from(Alias::new("tasks"))
			.and_where(Expr::cust("workspace_id = $1 AND parent_id = $2"))
			.to_string(PostgresQueryBuilder);
		let (has_pending, has_failed): (bool, bool) = sqlx::query_as(&query)
			.bind(workspace)
			.bind(parent)
			.fetch_one(&self.pool)
			.await?;
		Ok(ChildTaskSummary {
			has_pending,
			has_failed,
		})
	}
	pub async fn snapshot(&self, id: Uuid) -> Result<WorkspaceSnapshot> {
		Ok(WorkspaceSnapshot {
			workspace: self.workspace(id).await?,
			tasks: self.tasks(Some(id)).await?,
			artifacts: sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
					))
					.from(sea_orm::sea_query::Alias::new("artifacts"))
					.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
					.order_by_expr(
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
							sea_orm::sea_query::Alias::new("created_at"),
						)),
						sea_orm::sea_query::Order::Asc,
					)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.fetch_all(&self.pool)
			.await?,
			events: sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("sequence")),
					))
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
					))
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("node_id")),
					))
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
							"workspace_id",
						)),
					))
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("kind")),
					))
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("data")),
					))
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("created_at")),
					))
					.from_subquery(
						sea_orm::sea_query::Query::select()
							.expr(sea_orm::sea_query::SimpleExpr::from(
								sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
							))
							.from(sea_orm::sea_query::Alias::new("events"))
							.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
							.order_by_expr(
								sea_orm::sea_query::SimpleExpr::from(
									sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
										"sequence",
									)),
								),
								sea_orm::sea_query::Order::Desc,
							)
							.limit(100)
							.to_owned(),
						sea_orm::sea_query::Alias::new("e"),
					)
					.order_by_expr(
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
							sea_orm::sea_query::Alias::new("sequence"),
						)),
						sea_orm::sea_query::Order::Asc,
					)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.fetch_all(&self.pool)
			.await?,
			messages: sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
					))
					.from_subquery(
						sea_orm::sea_query::Query::select()
							.expr(sea_orm::sea_query::SimpleExpr::from(
								sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
							))
							.from(sea_orm::sea_query::Alias::new("messages"))
							.and_where(sea_orm::sea_query::Expr::cust("workspace_id = $1"))
							.order_by_expr(
								sea_orm::sea_query::SimpleExpr::from(
									sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new(
										"created_at",
									)),
								),
								sea_orm::sea_query::Order::Desc,
							)
							.limit(100)
							.to_owned(),
						sea_orm::sea_query::Alias::new("m"),
					)
					.order_by_expr(
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
							sea_orm::sea_query::Alias::new("created_at"),
						)),
						sea_orm::sea_query::Order::Asc,
					)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(id)
			.fetch_all(&self.pool)
			.await?,
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
		let mut tx = self.pool.begin().await?;
		let message = self
			.message_in(&mut tx, workspace, sender, content, key)
			.await?;
		tx.commit().await?;
		Ok(message)
	}
	pub(crate) async fn run_message_delivery_record(
		&self,
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
		let mut tx = self.pool.begin().await?;
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut *tx)
		.await?;
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		let reservation: Option<(Uuid, String, bool, bool, bool)> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.column(sea_orm::sea_query::Alias::new("run_id"))
				.column(sea_orm::sea_query::Alias::new("content"))
				.expr(sea_orm::sea_query::Expr::cust("expires_at IS NULL"))
				.expr(sea_orm::sea_query::Expr::cust(
					"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
				))
				.column(sea_orm::sea_query::Alias::new("consumed"))
				.from(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
				))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.bind(run_id)
		.bind(input_key)
		.fetch_optional(&mut *tx)
		.await?;
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
				task.status.as_str(),
				"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
			) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		// The database gate rejects the old home-write path during rolling
		// upgrades. Only the ledger-backed delivery endpoint sets this marker.
		sqlx::query_scalar::<_, String>(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"set_config('aidash.run_message_delivery', 'true', true)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
		let message = self
			.message_in(&mut tx, workspace, sender, content, Some(message_key))
			.await?;
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
				.value(
					sea_orm::sea_query::Alias::new("expires_at"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"task_id = $1 AND run_id = $2 AND idempotency_key = $3 AND content = $4",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.bind(run_id)
		.bind(input_key)
		.bind(content)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(message)
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
		let mut tx = self.pool.begin().await?;
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut *tx)
		.await?;
		let terminal = matches!(
			task.status.as_str(),
			"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
		);
		let full_message_key = format!("{peer_node}:{task_id}:{key}");
		let message_exists: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM messages WHERE workspace_id = $1 AND idempotency_key = $2 AND sender = $3 AND content = $4)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task.workspace_id)
		.bind(&full_message_key)
		.bind(format!("human@{peer_node}"))
		.bind(content)
		.fetch_one(&mut *tx)
		.await?;
		let previous: Option<(Uuid, String, bool, bool, bool)> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.column(sea_orm::sea_query::Alias::new("run_id"))
				.column(sea_orm::sea_query::Alias::new("content"))
				.expr(sea_orm::sea_query::Expr::cust("expires_at IS NULL"))
				.expr(sea_orm::sea_query::Expr::cust(
					"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
				))
				.column(sea_orm::sea_query::Alias::new("consumed"))
				.from(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"task_id = $1 AND idempotency_key = $2",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.bind(key)
		.fetch_optional(&mut *tx)
		.await?;
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
				sqlx::query(
					&sea_orm::sea_query::Query::update()
						.table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
						.value(
							sea_orm::sea_query::Alias::new("expires_at"),
							sea_orm::sea_query::Expr::cust("NULL"),
						)
						.and_where(sea_orm::sea_query::Expr::cust(
							"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(task_id)
				.bind(run_id)
				.bind(key)
				.execute(&mut *tx)
				.await?;
				tx.commit().await?;
				return Ok(());
			}
			if !committed && !active && !terminal {
				sqlx::query(
					&sea_orm::sea_query::Query::update()
						.table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
						.value(
							sea_orm::sea_query::Alias::new("expires_at"),
							sea_orm::sea_query::Expr::cust(
								"CURRENT_TIMESTAMP + INTERVAL '60 seconds'",
							),
						)
						.and_where(sea_orm::sea_query::Expr::cust(
							"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(task_id)
				.bind(run_id)
				.bind(key)
				.execute(&mut *tx)
				.await?;
			}
		} else {
			if terminal && !message_exists {
				return Err(Error::Conflict("home task is terminal".into()));
			}
			sqlx::query(
				&sea_orm::sea_query::Query::insert()
					.into_table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
					.columns([
						sea_orm::sea_query::Alias::new("task_id"),
						sea_orm::sea_query::Alias::new("run_id"),
						sea_orm::sea_query::Alias::new("idempotency_key"),
						sea_orm::sea_query::Alias::new("content"),
						sea_orm::sea_query::Alias::new("expires_at"),
					])
					.values_panic([
						sea_orm::sea_query::Expr::cust("$1"),
						sea_orm::sea_query::Expr::cust("$2"),
						sea_orm::sea_query::Expr::cust("$3"),
						sea_orm::sea_query::Expr::cust("$4"),
						if terminal {
							sea_orm::sea_query::Expr::cust("NULL")
						} else {
							sea_orm::sea_query::Expr::cust(
								"CURRENT_TIMESTAMP + INTERVAL '60 seconds'",
							)
						},
					])
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(task_id)
			.bind(run_id)
			.bind(key)
			.bind(content)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
		Ok(())
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
		if input_seq.is_some_and(|seq| seq <= 0) {
			return Err(Error::Invalid("invalid admitted input sequence".into()));
		}
		let mut tx = self.pool.begin().await?;
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut *tx)
		.await?;
		let reservation: Option<(String, bool, bool, Option<i64>)> = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.column(sea_orm::sea_query::Alias::new("content"))
				.expr(sea_orm::sea_query::Expr::cust("expires_at IS NULL"))
				.expr(sea_orm::sea_query::Expr::cust(
					"expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP",
				))
				.column(sea_orm::sea_query::Alias::new("input_seq"))
				.from(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"task_id = $1 AND run_id = $2 AND idempotency_key = $3",
				))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.bind(run_id)
		.bind(key)
		.fetch_optional(&mut *tx)
		.await?;
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
			tx.commit().await?;
			return Ok(());
		}
		// A run-bound acknowledgement from the delegated executor describes an
		// already persisted input, not a new admission. Its exact fence remains
		// recoverable after expiry or task termination; this never reopens the task.
		if !committed
			&& input_seq.is_none()
			&& (!active
				|| matches!(
					task.status.as_str(),
					"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
				)) {
			return Err(Error::Conflict(
				"remote run message reservation expired before admission was committed".into(),
			));
		}
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
				.value(
					sea_orm::sea_query::Alias::new("expires_at"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("input_seq"),
					sea_orm::sea_query::Expr::cust("COALESCE(input_seq, $5)"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"task_id = $1 AND run_id = $2 AND idempotency_key = $3 AND content = $4",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.bind(run_id)
		.bind(key)
		.bind(content)
		.bind(input_seq)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(())
	}
	pub async fn release_remote_run_message(
		&self,
		task_id: Uuid,
		run_id: Uuid,
		peer_node: &str,
		keys: &[String],
	) -> Result<()> {
		let mut tx = self.pool.begin().await?;
		// Deletion and task termination must use the same lock order.
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut *tx)
		.await?;
		for key in keys {
			let full_key = format!("{peer_node}:{task_id}:{key}");
			sqlx::query(
				&sea_orm::sea_query::Query::delete()
					.from_table(sea_orm::sea_query::Alias::new("remote_run_message_fences"))
					.and_where(sea_orm::sea_query::Expr::cust(
						"task_id = $1 AND run_id = $2 AND idempotency_key = $3 AND NOT consumed AND (expires_at IS NOT NULL OR input_seq IS NULL) AND NOT EXISTS (SELECT 1 FROM messages WHERE workspace_id = $4 AND idempotency_key = $5)",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(task_id)
			.bind(run_id)
			.bind(key)
			.bind(task.workspace_id)
			.bind(full_key)
			.execute(&mut *tx)
			.await?;
		}
		tx.commit().await?;
		Ok(())
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
		let mut tx = self.pool.begin().await?;
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut *tx)
		.await?;
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		if matches!(
			task.status.as_str(),
			"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
		) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		let pending: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND NOT consumed AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP))",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.bind(run_id)
		.fetch_one(&mut *tx)
		.await?;
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
		let mut tx = self.pool.begin().await?;
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.fetch_one(&mut *tx)
		.await?;
		if task.workspace_id != workspace {
			return Err(Error::Unauthorized);
		}
		if matches!(
			task.status.as_str(),
			"COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED"
		) {
			return Err(Error::Conflict("home task is terminal".into()));
		}
		let newer_input_pending: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND NOT consumed AND (expires_at IS NULL OR expires_at > CURRENT_TIMESTAMP) AND (input_seq IS NULL OR input_seq > $3))",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(task_id)
		.bind(run_id)
		.bind(included_input_seq)
		.fetch_one(&mut *tx)
		.await?;
		if newer_input_pending {
			return Err(Error::TransactionPending);
		}
		let _: String = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"set_config('aidash.input_ledger_worker', 'true', true)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_one(&mut *tx)
		.await?;
		let message = self
			.message_in(&mut tx, workspace, sender, content, Some(key))
			.await?;
		tx.commit().await?;
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
		let mut tx = self.pool.begin().await?;
		self.accept_run_message_in(&mut tx, run_id, sender, content, key, max_input_tokens)
			.await?;
		tx.commit().await?;
		Ok(())
	}
	pub(crate) async fn accept_run_message_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		run_id: Uuid,
		sender: &str,
		content: &str,
		key: &str,
		max_input_tokens: usize,
	) -> Result<()> {
		nonempty(content, "message")?;
		let run: Run = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.fetch_one(&mut **tx)
		.await?;
		let previous: Option<String> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.column(sea_orm::sea_query::Alias::new("content"))
				.from(sea_orm::sea_query::Alias::new("run_inputs"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"run_id = $1 AND idempotency_key = $2",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.bind(key)
		.fetch_optional(&mut **tx)
		.await?;
		if let Some(old_content) = previous {
			return if old_content == content {
				Ok(())
			} else {
				Err(Error::Conflict("run message idempotency key reused".into()))
			};
		}
		let task_status: Option<String> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.column(sea_orm::sea_query::Alias::new("status"))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.task_id)
		.fetch_optional(&mut **tx)
		.await?;
		if matches!(run.phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED")
			|| run.pending["finalizing"] == true
			|| run.control == "CANCELLED"
			|| run.pending["terminal_transition"].as_str().is_some()
			|| task_status.as_deref().is_some_and(|status| {
				matches!(status, "COMPLETED" | "FAILED" | "CANCELLED" | "ABANDONED")
			}) {
			return Err(Error::Conflict(
				"run message was not accepted because the run is completing or terminal".into(),
			));
		}
		if !run.ledger_worker_ready {
			if run.lease_owner.is_some() {
				return Err(Error::Conflict(
					"run is leased by a worker without input ledger fencing".into(),
				));
			}
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("runs"))
					.value(
						sea_orm::sea_query::Alias::new("ledger_worker_ready"),
						sea_orm::sea_query::Expr::value(true),
					)
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(run_id)
			.execute(&mut **tx)
			.await?;
		}
		let (_, used) = self
			.run_inputs_with_budget_in(tx, run_id, max_input_tokens)
			.await?;
		if used.saturating_add(run_input_size(sender, content)) > max_input_tokens {
			return Err(Error::Invalid(
				"run messages exceed the selected model's input limit".into(),
			));
		}
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("run_inputs"))
				.columns([
					sea_orm::sea_query::Alias::new("run_id"),
					sea_orm::sea_query::Alias::new("sender"),
					sea_orm::sea_query::Alias::new("content"),
					sea_orm::sea_query::Alias::new("idempotency_key"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
				])
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.bind(sender)
		.bind(content)
		.bind(key)
		.execute(&mut **tx)
		.await?;
		if run.home_node == self.node_id {
			let message = self
				.message_in(tx, run.workspace_id, sender, content, Some(key))
				.await?;
			self.bind_run_input_message_in(tx, run_id, key, message.id)
				.await?;
		}
		Ok(())
	}
	async fn run_inputs_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		run_id: Uuid,
	) -> Result<Vec<RunInput>> {
		Ok(sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.columns([
					sea_orm::sea_query::Alias::new("seq"),
					sea_orm::sea_query::Alias::new("sender"),
					sea_orm::sea_query::Alias::new("content"),
					sea_orm::sea_query::Alias::new("idempotency_key"),
					sea_orm::sea_query::Alias::new("message_id"),
					sea_orm::sea_query::Alias::new("reference_only"),
				])
				.from(sea_orm::sea_query::Alias::new("run_inputs"))
				.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
				.order_by(
					sea_orm::sea_query::Alias::new("seq"),
					sea_orm::sea_query::Order::Asc,
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.fetch_all(&mut **tx)
		.await?)
	}
	async fn run_inputs_with_budget_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		run_id: Uuid,
		max_input_tokens: usize,
	) -> Result<(Vec<RunInput>, usize)> {
		let mut inputs = self.run_inputs_in(tx, run_id).await?;
		let mut used = 0_usize;
		for input in &mut inputs {
			let full_size = run_input_size(&input.sender, &input.content);
			if !input.reference_only
				&& used.saturating_add(full_size) > max_input_tokens
				&& let Some(message_id) = input.message_id
			{
				// The first run-input migration could have backfilled content
				// before the selected model's limit was available. Reclassify under
				// the same run lock as admission instead of charging its full size.
				sqlx::query(
					&sea_orm::sea_query::Query::update()
						.table(sea_orm::sea_query::Alias::new("run_inputs"))
						.value(
							sea_orm::sea_query::Alias::new("reference_only"),
							sea_orm::sea_query::Expr::cust("TRUE"),
						)
						.and_where(sea_orm::sea_query::Expr::cust(
							"run_id = $1 AND idempotency_key = $2 AND message_id = $3",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(run_id)
				.bind(&input.idempotency_key)
				.bind(message_id)
				.execute(&mut **tx)
				.await?;
				input.reference_only = true;
			}
			if input.reference_only {
				let reference_size = input
					.message_id
					.map_or(usize::MAX, |id| run_input_reference_size(&input.sender, id));
				if reference_size > max_input_tokens {
					return Err(Error::Invalid(
						"run message reference exceeds the selected model's input limit".into(),
					));
				}
			} else {
				used = used.saturating_add(run_input_context_size(input));
			}
		}
		Ok((inputs, used))
	}
	pub async fn run_inputs(&self, run_id: Uuid) -> Result<Vec<RunInput>> {
		let mut tx = self.pool.begin().await?;
		let inputs = self.run_inputs_in(&mut tx, run_id).await?;
		tx.commit().await?;
		Ok(inputs)
	}
	pub(crate) async fn run_input_sequence(
		&self,
		run_id: Uuid,
		key: &str,
		content: &str,
	) -> Result<i64> {
		use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("seq"))
				.from(Alias::new("run_inputs"))
				.and_where(Expr::cust(
					"run_id = $1 AND idempotency_key = $2 AND content = $3",
				))
				.to_string(PostgresQueryBuilder),
		)
		.bind(run_id)
		.bind(key)
		.bind(content)
		.fetch_optional(&self.pool)
		.await?
		.ok_or_else(|| Error::Conflict("run message has no matching durable admission".into()))
	}
	pub(crate) async fn run_input_high_watermark(&self, run_id: Uuid) -> Result<i64> {
		use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
		Ok(sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COALESCE(MAX(seq), 0)"))
				.from(Alias::new("run_inputs"))
				.and_where(Expr::cust("run_id = $1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(run_id)
		.fetch_one(&self.pool)
		.await?)
	}
	pub async fn pending_terminal_run_message(&self) -> Result<Option<Run>> {
		Ok(sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"home_node <> $1 AND phase IN ('COMPLETED', 'FAILED', 'CANCELLED') AND EXISTS (SELECT 1 FROM run_inputs WHERE run_inputs.run_id = runs.id AND message_id IS NULL AND (delivery_retry_at IS NULL OR delivery_retry_at <= CURRENT_TIMESTAMP))",
				))
				.order_by(sea_orm::sea_query::Alias::new("updated_at"), sea_orm::sea_query::Order::Asc)
				.limit(1)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&self.node_id)
		.fetch_optional(&self.pool)
		.await?)
	}
	pub async fn defer_run_message_delivery(&self, run_id: Uuid) -> Result<()> {
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("run_inputs"))
				.value(
					sea_orm::sea_query::Alias::new("delivery_retry_at"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + INTERVAL '5 seconds'"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"run_id = $1 AND message_id IS NULL",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.execute(&self.pool)
		.await?;
		Ok(())
	}
	pub async fn bind_run_input_message(
		&self,
		run_id: Uuid,
		key: &str,
		message_id: Uuid,
	) -> Result<()> {
		let mut tx = self.pool.begin().await?;
		self.bind_run_input_message_in(&mut tx, run_id, key, message_id)
			.await?;
		tx.commit().await?;
		Ok(())
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
		let mut tx = self.pool.begin().await?;
		self.import_remote_run_messages_in(&mut tx, run_id, messages, max_input_tokens)
			.await?;
		tx.commit().await?;
		Ok(())
	}
	pub(crate) async fn import_remote_run_messages_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		run_id: Uuid,
		messages: &[(String, Message)],
		max_input_tokens: usize,
	) -> Result<()> {
		if messages.is_empty() {
			return Ok(());
		}
		// Hold the run-row lock for the whole fetched history batch. New admissions
		// use the same lock, so they cannot split older imports and overtake them.
		let current: Run = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.fetch_one(&mut **tx)
		.await?;
		for (key, message) in messages {
			self.import_remote_run_message_in(tx, run_id, &current, key, message, max_input_tokens)
				.await?;
		}
		Ok(())
	}
	/// Import the remote home's existing message history before admitting a new
	/// input, under one run-row lock and transaction so a new sequence cannot
	/// overtake a pre-ledger correction.
	pub async fn import_remote_run_messages_and_accept(
		&self,
		run_id: Uuid,
		messages: &[(String, Message)],
		sender: &str,
		content: &str,
		key: &str,
		max_input_tokens: usize,
	) -> Result<()> {
		let mut tx = self.pool.begin().await?;
		self.import_remote_run_messages_in(&mut tx, run_id, messages, max_input_tokens)
			.await?;
		self.accept_run_message_in(&mut tx, run_id, sender, content, key, max_input_tokens)
			.await?;
		tx.commit().await?;
		Ok(())
	}
	async fn import_remote_run_message_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		run_id: Uuid,
		current: &Run,
		key: &str,
		message: &Message,
		max_input_tokens: usize,
	) -> Result<()> {
		let (inputs, used) = self
			.run_inputs_with_budget_in(tx, run_id, max_input_tokens)
			.await?;
		let previous = inputs.iter().find(|input| input.idempotency_key == key);
		if previous.is_none()
			&& (current.pending["finalizing"] == true
				|| current.pending["terminal_transition"].as_str().is_some()
				|| current.control == "CANCELLED"
				|| matches!(current.phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED"))
		{
			return Err(Error::Conflict(
				"historical run message arrived after finalization".into(),
			));
		}
		let reference_only = previous.is_none()
			&& used.saturating_add(run_input_size(&message.sender, &message.content))
				> max_input_tokens;
		if previous.is_none()
			&& reference_only
			&& run_input_reference_size(&message.sender, message.id) > max_input_tokens
		{
			return Err(Error::Invalid(
				"historical run message reference exceeds the selected model's input limit".into(),
			));
		}
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("run_inputs"))
				.columns([
					sea_orm::sea_query::Alias::new("run_id"),
					sea_orm::sea_query::Alias::new("sender"),
					sea_orm::sea_query::Alias::new("content"),
					sea_orm::sea_query::Alias::new("idempotency_key"),
					sea_orm::sea_query::Alias::new("message_id"),
					sea_orm::sea_query::Alias::new("reference_only"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
					sea_orm::sea_query::Expr::cust("$6"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([
						sea_orm::sea_query::Alias::new("run_id"),
						sea_orm::sea_query::Alias::new("idempotency_key"),
					])
					.do_nothing()
					.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.bind(&message.sender)
		.bind(&message.content)
		.bind(key)
		.bind(message.id)
		.bind(reference_only)
		.execute(&mut **tx)
		.await?;
		let existing: String = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.column(sea_orm::sea_query::Alias::new("content"))
				.from(sea_orm::sea_query::Alias::new("run_inputs"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"run_id = $1 AND idempotency_key = $2",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.bind(key)
		.fetch_one(&mut **tx)
		.await?;
		if existing != message.content {
			return Err(Error::Conflict("historical run message key reused".into()));
		}
		self.bind_run_input_message_in(tx, run_id, key, message.id)
			.await?;
		Ok(())
	}

	async fn bind_run_input_message_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		run_id: Uuid,
		key: &str,
		message_id: Uuid,
	) -> Result<()> {
		let changed = sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("run_inputs"))
				.value(
					sea_orm::sea_query::Alias::new("message_id"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"run_id = $1 AND idempotency_key = $2 AND (message_id IS NULL OR message_id = $3)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run_id)
		.bind(key)
		.bind(message_id)
		.execute(&mut **tx)
		.await?;
		if changed.rows_affected() != 1 {
			return Err(Error::Conflict("run input message binding changed".into()));
		}
		Ok(())
	}
	pub async fn begin_final_completion(&self, run: &Run, worker: Uuid) -> Result<bool> {
		let mut tx = self.pool.begin().await?;
		let current: Run = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_until > CURRENT_TIMESTAMP",
				))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or_else(|| Error::Conflict("worker lease lost".into()))?;
		if current.lease_owner != Some(worker)
			|| current.phase != "TOOL_CALL"
			|| current.control == "CANCELLED"
			|| current.pending["terminal_transition"].as_str().is_some()
		{
			return Err(Error::Conflict("worker lease lost".into()));
		}
		let stale: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM run_inputs WHERE run_id = $1 AND seq > $2)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(run.observed_input_seq)
		.fetch_one(&mut *tx)
		.await?;
		if stale {
			return Ok(false);
		}
		let changed = sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("pending"),
					sea_orm::sea_query::Expr::cust("pending || '{\"finalizing\":true}'::jsonb"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(worker)
		.execute(&mut *tx)
		.await?;
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
		let mut tx = self.pool.begin().await?;
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
		tx: &mut Transaction<'_, Postgres>,
		run: Option<Uuid>,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		let Some(run) = run else {
			return Ok(());
		};
		let valid: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM authorization_execution WHERE run_id = $1 AND workspace_id = $2)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run)
		.bind(workspace)
		.fetch_one(&mut **tx)
		.await?;
		if !valid {
			return Err(Error::Forbidden);
		}
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("authorization_run_reads"))
				.columns([
					sea_orm::sea_query::Alias::new("run_id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("resource_kind"),
					sea_orm::sea_query::Alias::new("resource_id"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::new()
						.do_nothing()
						.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run)
		.bind(workspace)
		.bind(kind)
		.bind(id)
		.execute(&mut **tx)
		.await?;
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("authorization_run_outputs"))
				.columns([
					sea_orm::sea_query::Alias::new("run_id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("resource_kind"),
					sea_orm::sea_query::Alias::new("resource_id"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::new()
						.do_nothing()
						.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run)
		.bind(workspace)
		.bind(kind)
		.bind(id)
		.execute(&mut **tx)
		.await?;

		Ok(())
	}
	pub(crate) async fn message_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		workspace: Uuid,
		sender: &str,
		content: &str,
		key: Option<&str>,
	) -> Result<Message> {
		nonempty(content, "message")?;
		let inserted: Option<Message> = sqlx::query_as(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("messages"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("sender"),
					sea_orm::sea_query::Alias::new("content"),
					sea_orm::sea_query::Alias::new("idempotency_key"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new(
						"idempotency_key",
					)])
					.do_nothing()
					.to_owned(),
				)
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(Uuid::new_v4())
		.bind(workspace)
		.bind(sender)
		.bind(content)
		.bind(key)
		.fetch_optional(&mut **tx)
		.await?;
		let message = if let Some(message) = inserted {
			self.event(tx, Some(workspace), "message.created", json!(message))
				.await?;
			message
		} else {
			let message: Message = sqlx::query_as(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::SimpleExpr::from(
						sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
					))
					.from(sea_orm::sea_query::Alias::new("messages"))
					.and_where(sea_orm::sea_query::Expr::cust("idempotency_key = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(key)
			.fetch_one(&mut **tx)
			.await?;
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
		let generated: bool = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"EXISTS(SELECT 1 FROM generation_requests WHERE agent_id = $1 AND agent_version = $2)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(version)
		.fetch_one(&self.pool)
		.await?;
		if generated {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	pub(crate) async fn require_legacy_remote_task(&self, home: &str, task: Uuid) -> Result<()> {
		let admitted: bool = sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM authorization_remote_admissions WHERE source_node = $1 AND task_id = $2)")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(home).bind(task).fetch_one(&self.pool).await?;
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
		self.require_legacy_execution(task.workspace_id).await?;
		let mut tx = self.pool.begin().await?;
		// Serialize legacy admission against scoped receiver admission. Neither
		// mode may appear between the other's check and durable commit.
		sqlx::query(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"PG_ADVISORY_XACT_LOCK(HASHTEXTEXTENDED($1, 71003209))",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(format!("{home_node}:{}", task.id))
		.execute(&mut *tx)
		.await?;
		let admitted: bool = sqlx::query_scalar(&sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM authorization_remote_admissions WHERE source_node = $1 AND task_id = $2)")).to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(home_node).bind(task.id).fetch_one(&mut *tx).await?;
		if admitted {
			return Err(Error::Forbidden);
		}
		self.require_legacy_agent(agent_id, agent_version).await?;
		let row: Option<Run> = sqlx::query_as(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("runs"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("task_id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("home_node"),
					sea_orm::sea_query::Alias::new("agent_id"),
					sea_orm::sea_query::Alias::new("agent_version"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
					sea_orm::sea_query::Expr::cust("$6"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([
						sea_orm::sea_query::Alias::new("home_node"),
						sea_orm::sea_query::Alias::new("task_id"),
					])
					.do_nothing()
					.to_owned(),
				)
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(Uuid::new_v4())
		.bind(task.id)
		.bind(task.workspace_id)
		.bind(home_node)
		.bind(agent_id)
		.bind(agent_version)
		.fetch_optional(&mut *tx)
		.await?;
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
				let r: Run = sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("runs"))
						.and_where(sea_orm::sea_query::Expr::cust(
							"home_node = $1 AND task_id = $2",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(home_node)
				.bind(task.id)
				.fetch_one(&mut *tx)
				.await?;
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
		tx.commit().await?;
		Ok(run)
	}
	pub async fn run(&self, id: Uuid) -> Result<Run> {
		sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&self.pool)
		.await?
		.ok_or_else(|| Error::NotFound("run".into()))
	}
	pub async fn runs(&self) -> Result<Vec<Run>> {
		Ok(sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.order_by_expr(
					sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col(
						sea_orm::sea_query::Alias::new("updated_at"),
					)),
					sea_orm::sea_query::Order::Desc,
				)
				.limit(500)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_all(&self.pool)
		.await?)
	}
	pub async fn lease_run(&self, worker: Uuid, seconds: i32) -> Result<Option<Run>> {
		// SKIP LOCKED permits independent workers; the token fences stale writers.
		Ok(sqlx::query_as(&sea_orm::sea_query::Query::update().table(sea_orm::sea_query::Alias::new("runs")).value(sea_orm::sea_query::Alias::new("pending"), sea_orm::sea_query::Expr::cust("CASE WHEN lease_owner IS NOT NULL THEN pending || CAST('{\"lease_recovered\":true}' AS JSONB) ELSE pending END")).value(sea_orm::sea_query::Alias::new("lease_owner"), sea_orm::sea_query::Expr::cust("$1")).value(sea_orm::sea_query::Alias::new("lease_until"), sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + MAKE_INTERVAL(secs => $2)")).value(sea_orm::sea_query::Alias::new("revision"), sea_orm::sea_query::Expr::cust("revision + 1")).value(sea_orm::sea_query::Alias::new("ledger_worker_ready"), sea_orm::sea_query::Expr::value(true)).and_where(sea_orm::sea_query::Expr::cust("id = (SELECT id FROM runs WHERE NOT phase IN ('COMPLETED', 'FAILED', 'CANCELLED') AND control <> 'PAUSED' AND revision < 9223372036854775805 AND (lease_until IS NULL OR lease_until < CURRENT_TIMESTAMP) AND (NOT (pending ? 'retry_at') OR CAST((pending ->> 'retry_at') AS TIMESTAMPTZ) < CURRENT_TIMESTAMP) AND (phase <> 'WAITING' OR control = 'CANCELLED' OR CAST((pending ->> 'wake_at') AS TIMESTAMPTZ) < CURRENT_TIMESTAMP OR EXISTS(SELECT 1 FROM human_requests AS h WHERE CAST(h.id AS TEXT) = runs.pending ->> 'human_request_id' AND h.response IS NOT NULL)) ORDER BY updated_at LIMIT 1 FOR UPDATE SKIP LOCKED) AND set_config('aidash.input_ledger_worker', 'true', true) = 'true'")).returning_all().to_string(sea_orm::sea_query::PostgresQueryBuilder))
            .bind(worker).bind(seconds as f64).fetch_optional(&self.pool).await?)
	}
	pub async fn renew_lease(&self, id: Uuid, worker: Uuid, seconds: i32) -> Result<bool> {
		Ok(sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("lease_until"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + MAKE_INTERVAL(secs => $3)"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP AND set_config('aidash.input_ledger_worker', 'true', true) = 'true'",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(worker)
		.bind(seconds as f64)
		.execute(&self.pool)
		.await?
		.rows_affected()
			== 1)
	}
	pub async fn save_run(&self, run: &Run, worker: Uuid, kind: &str) -> Result<Run> {
		let mut pending = run.pending.clone();
		let retrying = matches!(kind, "run.retrying" | "run.failure_pending");
		if !retrying && let Some(object) = pending.as_object_mut() {
			object.remove("retry_count");
			object.remove("retry_at");
		}
		let error = if retrying || kind == "run.failed" {
			run.error.as_deref()
		} else {
			None
		};
		let mut tx = self.pool.begin().await?;
		if kind == "model.completed" {
			// Message admission takes this same row lock. A response is durable only
			// when every accepted input was present in its provider request.
			let _: Uuid = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.column(sea_orm::sea_query::Alias::new("id"))
					.from(sea_orm::sea_query::Alias::new("runs"))
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.lock(sea_orm::sea_query::LockType::Update)
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(run.id)
			.fetch_one(&mut *tx)
			.await?;
			let stale: bool = sqlx::query_scalar(
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust(
						"EXISTS(SELECT 1 FROM run_inputs WHERE run_id = $1 AND seq > $2)",
					))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(run.id)
			.bind(
				pending["included_input_seq"]
					.as_i64()
					.unwrap_or(run.observed_input_seq),
			)
			.fetch_one(&mut *tx)
			.await?;
			if stale {
				return Err(Error::StaleInference);
			}
		}
		let saved: Run = sqlx::query_as(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("phase"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.value(
					sea_orm::sea_query::Alias::new("context"),
					sea_orm::sea_query::Expr::cust("$4"),
				)
				.value(
					sea_orm::sea_query::Alias::new("pending"),
					sea_orm::sea_query::Expr::cust("$5"),
				)
				.value(
					sea_orm::sea_query::Alias::new("step"),
					sea_orm::sea_query::Expr::cust("$6"),
				)
				.value(
					sea_orm::sea_query::Alias::new("error"),
					sea_orm::sea_query::Expr::cust("$7"),
				)
			.value(
				sea_orm::sea_query::Alias::new("revision"),
				sea_orm::sea_query::Expr::cust("revision + 1"),
			)
			.value(
				sea_orm::sea_query::Alias::new("observed_input_seq"),
				sea_orm::sea_query::Expr::cust("$9"),
			)
				.value(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
				)
				.value(
					sea_orm::sea_query::Alias::new("lease_owner"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("lease_until"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP AND ($8 OR control <> 'CANCELLED')",
				))
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(worker)
		.bind(&run.phase)
		.bind(&run.context)
		.bind(&pending)
		.bind(run.step)
		.bind(error)
		.bind(kind != "model.completed")
		.bind(run.observed_input_seq)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or_else(|| Error::Conflict("worker lease lost".into()))?;
		self.event(&mut tx, (run.home_node == self.node_id).then_some(run.workspace_id), kind,
            json!({"run_id":saved.id,"task_id":saved.task_id,"workspace_id":saved.workspace_id,"agent_id":saved.agent_id,"phase":saved.phase,"step":saved.step,"error":saved.error,"context_usage":saved.context.get("usage")})).await?;
		tx.commit().await?;
		Ok(saved)
	}
	pub async fn release_lease(&self, id: Uuid, worker: Uuid) -> Result<()> {
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("lease_owner"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("lease_until"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(worker)
		.execute(&self.pool)
		.await?;
		Ok(())
	}
	pub(crate) async fn pause_for_authorization(
		&self,
		run: &Run,
		worker: Uuid,
		reason: &str,
	) -> Result<()> {
		let mut tx = self.pool.begin().await?;
		let changed = sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("control"),
					sea_orm::sea_query::Expr::cust(
						"CASE WHEN control = 'CANCELLED' THEN control ELSE 'PAUSED' END",
					),
				)
				.value(
					sea_orm::sea_query::Alias::new("error"),
					sea_orm::sea_query::Expr::cust("CASE WHEN control = 'PAUSED' AND error IS DISTINCT FROM 'identity status unavailable' THEN error ELSE $3 END"),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.value(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
				)
				.value(
					sea_orm::sea_query::Alias::new("lease_owner"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("lease_until"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(worker)
		.bind(reason)
		.execute(&mut *tx)
		.await?
		.rows_affected();
		if changed == 0 {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		self.event(
			&mut tx,
			(run.home_node == self.node_id).then_some(run.workspace_id),
			"run.authorization_blocked",
			json!({"run_id":run.id,"task_id":run.task_id}),
		)
		.await?;
		tx.commit().await?;
		Ok(())
	}
	pub(crate) async fn cancel_execution(&self, run: &Run, worker: Uuid) -> Result<()> {
		let mut tx = self.pool.begin().await?;
		let valid: Option<Uuid> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
				))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP AND control = 'CANCELLED'",
				))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(worker)
		.fetch_optional(&mut *tx)
		.await?;
		if valid.is_none() {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		let task: Task = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("tasks"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.task_id)
		.fetch_one(&mut *tx)
		.await?;
		let phase = if task.status == "COMPLETED" {
			"COMPLETED"
		} else {
			"CANCELLED"
		};
		if !matches!(
			task.status.as_str(),
			"COMPLETED" | "CANCELLED" | "ABANDONED"
		) {
			let task: Task = sqlx::query_as(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("tasks"))
					.value(
						sea_orm::sea_query::Alias::new("status"),
						sea_orm::sea_query::Expr::cust("'CANCELLED'"),
					)
					.value(
						sea_orm::sea_query::Alias::new("revision"),
						sea_orm::sea_query::Expr::cust("revision + 1"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.returning_all()
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(run.task_id)
			.fetch_one(&mut *tx)
			.await?;
			self.event(&mut tx, Some(run.workspace_id), "task.updated", json!(task))
				.await?;
		}
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("phase"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.value(
					sea_orm::sea_query::Alias::new("pending"),
					sea_orm::sea_query::Expr::cust("'{}'"),
				)
				.value(
					sea_orm::sea_query::Alias::new("error"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.value(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
				)
				.value(
					sea_orm::sea_query::Alias::new("lease_owner"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.value(
					sea_orm::sea_query::Alias::new("lease_until"),
					sea_orm::sea_query::Expr::cust("NULL"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(worker)
		.bind(phase)
		.execute(&mut *tx)
		.await?;
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
	pub async fn control(&self, id: Uuid, action: &str) -> Result<Run> {
		let mut tx = self.pool.begin().await?;
		let run = self.control_in(&mut tx, id, action).await?;
		tx.commit().await?;
		Ok(run)
	}
	pub(crate) async fn control_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		id: Uuid,
		action: &str,
	) -> Result<Run> {
		let control = match action {
			"pause" => "PAUSED",
			"resume" => "ACTIVE",
			"cancel" => "CANCELLED",
			_ => {
				return Err(Error::Invalid(
					"control must be pause, resume or cancel".into(),
				));
			}
		};
		// Control-plane updates are emitted by upgraded code and must remain
		// available while a pre-upgrade worker lease is fenced by run_inputs.
		sqlx::query_scalar::<_, String>(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::Expr::cust(
					"set_config('aidash.input_ledger_worker', 'true', true)",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.fetch_one(&mut **tx)
		.await?;
		let r: Run = sqlx::query_as(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("control"),
					sea_orm::sea_query::Expr::cust("$2"),
				)
				.value(
					sea_orm::sea_query::Alias::new("error"),
					sea_orm::sea_query::Expr::cust(
						"CASE WHEN $2 = 'PAUSED' AND error = 'identity status unavailable' THEN NULL ELSE error END",
					),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.value(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND NOT phase IN ('COMPLETED', 'FAILED', 'CANCELLED') AND control <> 'CANCELLED'",
				))
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(control)
		.fetch_optional(&mut **tx)
		.await?
		.ok_or_else(|| {
			Error::Conflict("run is terminal or cancellation is already requested".into())
		})?;
		self.event(
			tx,
			(r.home_node == self.node_id).then_some(r.workspace_id),
			"run.control",
			json!({"run_id":id,"action":action}),
		)
		.await?;
		Ok(r)
	}
	pub async fn human_request(
		&self,
		run: &Run,
		kind: &str,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		let mut tx = self.pool.begin().await?;
		let request = self
			.human_request_in(&mut tx, run, kind, prompt, key)
			.await?;
		tx.commit().await?;
		Ok(request)
	}

	async fn human_request_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
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
		let h: Option<HumanRequest> = sqlx::query_as(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("human_requests"))
				.columns([
					sea_orm::sea_query::Alias::new("id"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("run_id"),
					sea_orm::sea_query::Alias::new("kind"),
					sea_orm::sea_query::Alias::new("prompt"),
					sea_orm::sea_query::Alias::new("request_key"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
					sea_orm::sea_query::Expr::cust("$6"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([sea_orm::sea_query::Alias::new(
						"request_key",
					)])
					.do_nothing()
					.to_owned(),
				)
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(Uuid::new_v4())
		.bind(run.workspace_id)
		.bind(run.id)
		.bind(kind)
		.bind(prompt)
		.bind(key)
		.fetch_optional(&mut **tx)
		.await?;
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
				sqlx::query_as(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::SimpleExpr::from(
							sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
						))
						.from(sea_orm::sea_query::Alias::new("human_requests"))
						.and_where(sea_orm::sea_query::Expr::cust("request_key = $1"))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(key)
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
	pub(crate) async fn reconciliation_request(
		&self,
		run: &mut Run,
		worker: Uuid,
		key: &str,
		prompt: &str,
	) -> Result<()> {
		let mut tx = self.pool.begin().await?;
		let h = self
			.human_request_in(
				&mut tx,
				run,
				"CONFIRMATION",
				prompt,
				&format!("{key}:reconcile"),
			)
			.await?;
		run.pending["human_request_id"] = json!(h.id);
		run.pending["uncertain_key"] = json!(key);
		run.pending["resume_phase"] = json!("TOOL_CALL");
		run.phase = "WAITING".into();
		let changed = sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("pending"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.value(
					sea_orm::sea_query::Alias::new("phase"),
					sea_orm::sea_query::Expr::cust("'WAITING'"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(worker)
		.bind(&run.pending)
		.execute(&mut *tx)
		.await?
		.rows_affected();
		if changed != 1 {
			return Err(Error::Conflict("worker lease lost".into()));
		}
		tx.commit().await?;
		Ok(())
	}
	pub async fn answer(&self, id: Uuid, response: Value) -> Result<HumanRequest> {
		let mut tx = self.pool.begin().await?;
		let request = self.answer_in(&mut tx, id, response, "human").await?;
		tx.commit().await?;
		Ok(request)
	}
	pub(crate) async fn answer_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		id: Uuid,
		response: Value,
		actor: &str,
	) -> Result<HumanRequest> {
		if response.is_null() {
			return Err(Error::Invalid("a human response cannot be null".into()));
		}
		let old: HumanRequest = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("human_requests"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_optional(&mut **tx)
		.await?
		.ok_or_else(|| Error::NotFound("human request".into()))?;
		if let Some(existing) = &old.response {
			if existing != &response {
				return Err(Error::Conflict("human request already answered".into()));
			}
			return Ok(old);
		}
		let run: Run = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(old.run_id)
		.fetch_one(&mut **tx)
		.await?;
		if run
			.pending
			.get("uncertain_key")
			.and_then(Value::as_str)
			.is_some()
			&& response.get("result").is_none()
		{
			return Err(Error::Invalid(
				"tool reconciliation requires a JSON object containing result".into(),
			));
		}
		let h: HumanRequest = sqlx::query_as(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("human_requests"))
				.value(
					sea_orm::sea_query::Alias::new("response"),
					sea_orm::sea_query::Expr::cust("$2"),
				)
				.value(
					sea_orm::sea_query::Alias::new("answered_by"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.returning_all()
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(id)
		.bind(response)
		.bind(actor)
		.fetch_one(&mut **tx)
		.await?;
		self.event(
			tx,
			(run.home_node == self.node_id).then_some(run.workspace_id),
			"human.answered",
			json!(h),
		)
		.await?;
		Ok(h)
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
		let mut tx = self.pool.begin().await?;
		self.ensure_run_response_current_in(
			&mut tx,
			run.id,
			worker,
			run.pending["included_input_seq"]
				.as_i64()
				.unwrap_or(run.observed_input_seq),
		)
		.await?;
		// The bounded call and any prepared workspace-read chunk must survive a
		// worker crash once the idempotency key becomes durable. Keep this update
		// in the same transaction as invocation creation.
		let persisted = sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("pending"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.value(
					sea_orm::sea_query::Alias::new("revision"),
					sea_orm::sea_query::Expr::cust("revision + 1"),
				)
				.value(
					sea_orm::sea_query::Alias::new("updated_at"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(worker)
		.bind(&run.pending)
		.execute(&mut *tx)
		.await?
		.rows_affected();
		if persisted != 1 {
			return Err(Error::Conflict(
				"worker lease lost before persisting tool input".into(),
			));
		}
		let created = sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("invocations"))
				.columns([
					sea_orm::sea_query::Alias::new("idempotency_key"),
					sea_orm::sea_query::Alias::new("run_id"),
					sea_orm::sea_query::Alias::new("tool"),
					sea_orm::sea_query::Alias::new("input"),
					sea_orm::sea_query::Alias::new("status"),
					sea_orm::sea_query::Alias::new("replay_safe"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("'STARTED'"),
					sea_orm::sea_query::Expr::cust("$5"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::new()
						.do_nothing()
						.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(key)
		.bind(run.id)
		.bind(tool)
		.bind(input)
		.bind(replay_safe)
		.execute(&mut *tx)
		.await?
		.rows_affected()
			== 1;
		let mut invocation: Invocation = sqlx::query_as(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Asterisk),
				))
				.from(sea_orm::sea_query::Alias::new("invocations"))
				.and_where(sea_orm::sea_query::Expr::cust("idempotency_key = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(key)
		.fetch_one(&mut *tx)
		.await?;
		if invocation.input != *input || invocation.tool != tool || invocation.run_id != run.id {
			return Err(Error::Conflict(
				"tool idempotency key reused with different input".into(),
			));
		}
		if !created && invocation.status != "COMPLETED" && !invocation.replay_safe {
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("invocations"))
					.value(
						sea_orm::sea_query::Alias::new("status"),
						sea_orm::sea_query::Expr::cust("'UNCERTAIN'"),
					)
					.and_where(sea_orm::sea_query::Expr::cust("idempotency_key = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(key)
			.execute(&mut *tx)
			.await?;
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
		let mut tx = self.pool.begin().await?;
		let valid: Option<Uuid> = sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("id")),
				))
				.from(sea_orm::sea_query::Alias::new("runs"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"id = $1 AND lease_owner = $2 AND lease_until > CURRENT_TIMESTAMP",
				))
				.lock(sea_orm::sea_query::LockType::Update)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.bind(worker)
		.fetch_optional(&mut *tx)
		.await?;
		if valid.is_none() {
			return Err(Error::Conflict(
				"worker lease lost while recording tool result".into(),
			));
		}
		let changed = sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("invocations"))
				.value(
					sea_orm::sea_query::Alias::new("status"),
					sea_orm::sea_query::Expr::cust("'COMPLETED'"),
				)
				.value(
					sea_orm::sea_query::Alias::new("result"),
					sea_orm::sea_query::Expr::cust("$3"),
				)
				.and_where(sea_orm::sea_query::Expr::cust(
					"idempotency_key = $1 AND run_id = $2 AND status <> 'COMPLETED'",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(key)
		.bind(run.id)
		.bind(result)
		.execute(&mut *tx)
		.await?
		.rows_affected();
		if changed > 0 {
			self.event(
				&mut tx,
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
	pub(crate) fn memory_home<'a>(&self, run: &'a Run) -> &'a str {
		if run.home_node == self.node_id {
			""
		} else {
			&run.home_node
		}
	}
	pub async fn remember(&self, run: &Run, data: &Value) -> Result<()> {
		let mut tx = self.pool.begin().await?;
		self.remember_in(&mut tx, run, data).await?;
		tx.commit().await?;
		Ok(())
	}
	pub(crate) async fn remember_in(
		&self,
		tx: &mut Transaction<'_, Postgres>,
		run: &Run,
		data: &Value,
	) -> Result<()> {
		sqlx::query(
			&sea_orm::sea_query::Query::insert()
				.into_table(sea_orm::sea_query::Alias::new("memory"))
				.columns([
					sea_orm::sea_query::Alias::new("agent_id"),
					sea_orm::sea_query::Alias::new("agent_version"),
					sea_orm::sea_query::Alias::new("workspace_id"),
					sea_orm::sea_query::Alias::new("data"),
					sea_orm::sea_query::Alias::new("home_node"),
				])
				.values_panic([
					sea_orm::sea_query::Expr::cust("$1"),
					sea_orm::sea_query::Expr::cust("$2"),
					sea_orm::sea_query::Expr::cust("$3"),
					sea_orm::sea_query::Expr::cust("$4"),
					sea_orm::sea_query::Expr::cust("$5"),
				])
				.on_conflict(
					sea_orm::sea_query::OnConflict::columns([
						sea_orm::sea_query::Alias::new("agent_id"),
						sea_orm::sea_query::Alias::new("agent_version"),
						sea_orm::sea_query::Alias::new("workspace_id"),
						sea_orm::sea_query::Alias::new("home_node"),
					])
					.value(
						sea_orm::sea_query::Alias::new("data"),
						sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((
							sea_orm::sea_query::Alias::new("excluded"),
							sea_orm::sea_query::Alias::new("data"),
						))),
					)
					.to_owned(),
				)
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&run.agent_id)
		.bind(&run.agent_version)
		.bind(run.workspace_id)
		.bind(data)
		.bind(self.memory_home(run))
		.execute(&mut **tx)
		.await?;
		Ok(())
	}
	pub async fn memory(&self, run: &Run) -> Result<Value> {
		Ok(sqlx::query_scalar(
			&sea_orm::sea_query::Query::select()
				.expr(sea_orm::sea_query::SimpleExpr::from(
					sea_orm::sea_query::Expr::col(sea_orm::sea_query::Alias::new("data")),
				))
				.from(sea_orm::sea_query::Alias::new("memory"))
				.and_where(sea_orm::sea_query::Expr::cust(
					"agent_id = $1 AND agent_version = $2 AND workspace_id = $3 AND home_node = $4",
				))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(&run.agent_id)
		.bind(&run.agent_version)
		.bind(run.workspace_id)
		.bind(self.memory_home(run))
		.fetch_optional(&self.pool)
		.await?
		.unwrap_or_else(empty_object))
	}
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Invocation {
	pub idempotency_key: String,
	pub run_id: Uuid,
	pub tool: String,
	pub input: Value,
	pub status: String,
	pub result: Option<Value>,
	pub replay_safe: bool,
	pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Dashboard journals expose bounded previews; durable invocation records keep
/// the original values for recovery and execution.
pub(crate) fn invocation_summary(alias: Option<&str>) -> sea_orm::sea_query::SelectStatement {
	use sea_orm::sea_query::{Alias, Expr, Query};
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
