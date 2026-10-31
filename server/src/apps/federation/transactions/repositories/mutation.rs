//! Native mutation primitives borrow the participant's original transaction.
use crate::apps::{
	execution::{
		models::{Run, event_records},
		repositories::inference,
	},
	federation::remote::models::Delegation,
	identity::models::{AuthorizationExecution, AuthorizationRunOutput},
	workspaces::models::{Task as TaskRecord, Workspace},
};
use aidash_application::{Result, ports::transactions::mutation::MutationScope};
use aidash_domain::{
	Artifact, ArtifactInput, Task, Workspace as WorkspaceContract,
	provider::progress::ProgressOutcome, run_state::RawRun,
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use serde_json::Value;
use uuid::Uuid;

/// Borrows the participant transaction. Interruptions written here are recorded
/// by the owner only after that transaction commits.
pub(crate) struct Scope<'a>(
	pub(crate) &'a mut dyn TransactionExecutor,
	pub(crate) Vec<ProgressOutcome>,
	pub(crate) String,
);

#[async_trait]
impl MutationScope for Scope<'_> {
	async fn replace_workspace(
		&mut self,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<WorkspaceContract> {
		require_subnode_writable(self.0, "workspace", id).await?;
		require_target_node(self.0, &self.2, "workspace", id).await?;
		Workspace::replace_state(self.0, id, revision, state)
			.await
			.map_err(Into::into)
	}
	async fn lock_task(&mut self, id: Uuid) -> Result<Task> {
		require_subnode_writable(self.0, "task", id).await?;
		require_target_node(self.0, &self.2, "task", id).await?;
		TaskRecord::lock(self.0, id)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn unfinished_children(&mut self, id: Uuid) -> Result<bool> {
		TaskRecord::unfinished_children(self.0, id)
			.await
			.map_err(Into::into)
	}
	async fn delegated_node(&mut self, task: Uuid) -> Result<Option<String>> {
		Delegation::node_for_task(self.0, task)
			.await
			.map_err(Into::into)
	}
	async fn complete_task(
		&mut self,
		task: &Task,
		artifact: &ArtifactInput,
		key: &str,
	) -> Result<(Task, Artifact)> {
		TaskRecord::complete(self.0, task, artifact, key)
			.await
			.map_err(Into::into)
	}
	async fn source_run(&mut self, task: Uuid) -> Result<Option<Uuid>> {
		AuthorizationExecution::source_run_for_task(self.0, task)
			.await
			.map_err(Into::into)
	}
	async fn record_output(
		&mut self,
		run: Uuid,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<()> {
		AuthorizationRunOutput::record(self.0, run, workspace, kind, id)
			.await
			.map_err(Into::into)
	}
	async fn lock_run(&mut self, id: Uuid) -> Result<(RawRun, bool)> {
		require_subnode_writable(self.0, "run", id).await?;
		require_target_node(self.0, &self.2, "run", id).await?;
		let (record, leased) = Run::lock_with_lease(self.0, id).await?;
		// RawRun preserves JSON context and pending without decoding execution state.
		Ok((
			serde_json::from_value(serde_json::to_value(record)?)?,
			leased,
		))
	}
	async fn complete_run(&mut self, id: Uuid) -> Result<()> {
		Run::complete(self.0, id).await?;
		aidash_application::subnodes::control_for_creator(
			&mut crate::apps::execution::repositories::subnodes::NativeCascade { tx: self.0 },
			id,
			aidash_domain::RunControlAction::Cancel,
			aidash_domain::ControlSource::System,
			Some(aidash_domain::subnodes::EndReason::CreatorCompleted),
		)
		.await?;
		// Completion is validated as Home-local, so the Home node records markers.
		let run = inference::target(&mut *self.0, id).await?;
		let closed = inference::close_pending(
			self.0,
			aidash_domain::nodes::deployment_of(&run.home_node),
			id,
			None,
		)
		.await?;
		self.1.extend(closed);
		Ok(())
	}
	async fn append_event(
		&mut self,
		workspace: Option<Uuid>,
		kind: &str,
		data: Value,
	) -> Result<()> {
		// An event with no Workspace is a Run's end on a Node that is not its Home,
		// and belongs to the Run's Tenant.
		match (
			workspace,
			data.get("run_id").and_then(Value::as_str).map(str::parse),
		) {
			(None, Some(Ok(run))) => event_records::append_for_run(self.0, run, kind, data).await,
			_ => event_records::append(self.0, workspace, kind, data).await,
		}
		.map_err(Into::into)
	}
}

/// A distributed mutation must not race dissolution after its preflight.
/// RLS restricts both the target and the Subnode to the current Tenant.
async fn require_subnode_writable(
	tx: &mut dyn TransactionExecutor,
	kind: &str,
	id: Uuid,
) -> Result<()> {
	use reinhardt::db::orm::{QueryRow, execution::convert_values};
	use reinhardt::query::{
		Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	let mut query = Query::select();
	query
		.column(Alias::new("state"))
		.from(Alias::new("subnodes"));
	if kind == "workspace" {
		query.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(id)));
	} else {
		let table = match kind {
			"task" => "tasks",
			"run" => "runs",
			_ => return Err(aidash_application::Error::Forbidden),
		};
		let target = Query::select()
			.column(Alias::new("workspace_id"))
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.to_owned();
		query.and_where(Expr::col(Alias::new("workspace_id")).in_subquery(target));
	}
	let (sql, values) = query.lock(LockType::Share).build(PostgresQueryBuilder);
	if let Some(row) = tx
		.fetch_optional(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?
	{
		let row = QueryRow::from_backend_row(row);
		let state: String = row.get("state").ok_or(crate::Error::Forbidden)?;
		if state != "active" {
			return Err(aidash_application::Error::Conflict(
				"Subnode is dissolved".into(),
			));
		}
	}
	Ok(())
}

/// A participant may mutate only records stored on its exact Node. Hold SHARE
/// locks through preflight/validation so ownership cannot change beneath it.
pub(super) async fn require_target_node(
	tx: &mut dyn TransactionExecutor,
	node: &str,
	kind: &str,
	id: Uuid,
) -> Result<()> {
	use reinhardt::db::orm::execution::convert_values;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, JoinType, LockType, PostgresQueryBuilder, Query,
		QueryStatementBuilder, TableRef,
	};
	let node = aidash_domain::nodes::NodeAddress::parse(node)?;
	let table = match kind {
		"workspace" => "workspaces",
		"task" => "tasks",
		"run" => "runs",
		_ => {
			return Err(aidash_application::Error::Invalid(
				"unsupported transaction target".into(),
			));
		}
	};
	let mut query = Query::select();
	query
		.expr(Expr::value(true))
		.from_as(Alias::new(table), Alias::new("target"))
		.and_where(Expr::col((Alias::new("target"), Alias::new("id"))).eq(Expr::value(id)));
	let owner = if kind == "task" {
		query.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("workspaces"), Alias::new("owner")),
			Expr::col((Alias::new("owner"), Alias::new("id"))).eq(Expr::col((
				Alias::new("target"),
				Alias::new("workspace_id"),
			))),
		);
		"owner"
	} else {
		"target"
	};
	query
		.and_where(
			Expr::col((Alias::new(owner), Alias::new("node"))).eq(Expr::value(node.node.as_str())),
		)
		.lock(LockType::Share);
	let (sql, values) = query.build(PostgresQueryBuilder);
	if tx
		.fetch_optional(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?
		.is_none()
	{
		return Err(aidash_application::Error::Forbidden);
	}
	Ok(())
}
