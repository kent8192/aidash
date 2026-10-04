//! Workspace-owned reads and bounded public task discovery.
use super::{Artifact, Message, Task, Workspace};
use crate::apps::workspaces::serializers::{
	entities::{Artifact as ArtifactContract, Message as MessageContract, Task as TaskContract},
	services::ChildTaskSummary,
	tasks::TaskPage,
};
use crate::{Error, Result};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor, QueryRow};
use reinhardt::query::{
	Alias, Expr, ExprTrait, Func, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;

impl Workspace {
	pub(crate) async fn read<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Self> {
		Self::objects()
			.filter(Self::field_id().eq(id))
			.first_with_db(db)
			.await?
			.ok_or_else(|| Error::NotFound("workspace".into()))
	}

	pub(crate) async fn newest_first<E: OrmExecutor>(db: &mut E) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.all()
			.order_by(&["-created_at", "-id"])
			.all_with_db(db)
			.await?)
	}

	/// Serialize the public contract without exposing model-only relation columns.
	pub(crate) async fn record<E: OrmExecutor>(
		db: &mut E,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Result<Value> {
		let record = match kind {
			"task" => Task::objects()
				.filter(Task::field_id().eq(id))
				.filter(Task::field_workspace_id().eq(workspace))
				.first_with_db(db)
				.await?
				.map(|row| json!(TaskContract::from(row))),
			"artifact" => Artifact::objects()
				.filter(Artifact::field_id().eq(id))
				.filter(Artifact::field_workspace_id().eq(workspace))
				.first_with_db(db)
				.await?
				.map(|row| json!(ArtifactContract::from(row))),
			"message" => Message::objects()
				.filter(Message::field_id().eq(id))
				.filter(Message::field_workspace_id().eq(workspace))
				.first_with_db(db)
				.await?
				.map(|row| json!(MessageContract::from(row))),
			_ => return Err(Error::Invalid("unknown workspace record kind".into())),
		};
		record.ok_or_else(|| Error::Invalid("workspace record not available".into()))
	}
}

impl Task {
	pub(crate) async fn read<E: OrmExecutor>(db: &mut E, id: Uuid) -> Result<Self> {
		Self::objects()
			.filter(Self::field_id().eq(id))
			.first_with_db(db)
			.await?
			.ok_or_else(|| Error::NotFound("task".into()))
	}

	pub(crate) async fn chronological<E: OrmExecutor>(
		db: &mut E,
		workspace: Option<Uuid>,
	) -> Result<Vec<Self>> {
		let mut query = Self::objects().all().order_by(&["created_at", "id"]);
		if let Some(workspace) = workspace {
			query = query.filter(Self::field_workspace_id().eq(workspace));
		}
		Ok(query.all_with_db(db).await?)
	}

	pub(crate) async fn page<E: OrmExecutor>(db: &mut E, offset: u64) -> Result<TaskPage> {
		let start = usize::try_from(offset)
			.map_err(|_| Error::Invalid("task offset exceeds platform limits".into()))?;
		let tasks: Vec<TaskContract> = Self::objects()
			.all()
			.order_by(&["-created_at", "-id"])
			.limit(500)
			.offset(start)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(Into::into)
			.collect();
		Ok(TaskPage {
			next_offset: (tasks.len() == 500).then_some(offset.saturating_add(500)),
			tasks,
		})
	}

	pub(crate) async fn child_summary<E: OrmExecutor>(
		db: &mut E,
		workspace: Uuid,
		parent: Uuid,
	) -> Result<ChildTaskSummary> {
		let any = |predicate| {
			Func::coalesce(vec![
				SimpleExpr::FunctionCall("bool_or".into_iden(), vec![predicate]),
				Expr::value(false).into(),
			])
		};
		let (sql, values) = Query::select()
			.expr_as(
				any(Expr::col("status").is_not_in(["COMPLETED", "ABANDONED"])),
				Alias::new("has_pending"),
			)
			.expr_as(
				any(Expr::col("status").is_in(["FAILED", "BLOCKED", "CANCELLED"])),
				Alias::new("has_failed"),
			)
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
			.and_where(Expr::col("parent_id").eq(Expr::value(parent)))
			.build(PostgresQueryBuilder);
		let row = db.fetch_one(&sql, convert_values(values)).await?;
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(row).data,
		)?)
	}
}

impl Artifact {
	pub(crate) async fn for_workspace<E: OrmExecutor>(
		db: &mut E,
		workspace: Uuid,
	) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_workspace_id().eq(workspace))
			.order_by(&["created_at", "id"])
			.all_with_db(db)
			.await?)
	}
}

impl Message {
	pub(crate) async fn recent<E: OrmExecutor>(db: &mut E, workspace: Uuid) -> Result<Vec<Self>> {
		let mut messages = Self::objects()
			.filter(Self::field_workspace_id().eq(workspace))
			.order_by(&["-created_at", "-id"])
			.limit(100)
			.all_with_db(db)
			.await?;
		messages.reverse();
		Ok(messages)
	}
}
