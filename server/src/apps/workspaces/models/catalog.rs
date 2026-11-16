//! Workspace-owned reads and bounded public task discovery.
use super::{Artifact, Message, Task, Workspace, child_flags};
use crate::apps::workspaces::serializers::{
	entities::{Artifact as ArtifactContract, Message as MessageContract, Task as TaskContract},
	services::ChildTaskSummary,
	tasks::TaskPage,
};
use crate::{Error, Result};
use aidash_domain::unassigned::{PendingChild, classify};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor, QueryRow};
use reinhardt::query::{
	Alias, Expr, ExprTrait, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
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
			"message" => {
				let row = Message::objects()
					.filter(Message::field_id().eq(id))
					.filter(Message::field_workspace_id().eq(workspace))
					.first_with_db(db)
					.await?;
				match row {
					Some(row) => Some(message_record(db, row).await?),
					None => None,
				}
			}
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
			unconfirmed: vec![],
		})
	}

	pub(crate) async fn child_summary<E: OrmExecutor>(
		db: &mut E,
		workspace: Uuid,
		parent: Uuid,
	) -> Result<ChildTaskSummary> {
		#[derive(serde::Deserialize)]
		struct Child {
			revision: i64,
			id: Uuid,
			title: String,
			status: crate::domain::TaskStatus,
			#[serde(default)]
			dependencies: Vec<Uuid>,
			started: bool,
			abandonable: bool,
		}
		let mut summary = ChildTaskSummary::default();
		// Which pending child is unassigned depends on its siblings, which may
		// sit on a later page: classify once, after the last page.
		let mut pending = Vec::new();
		let mut after = None;
		loop {
			let mut query = Query::select();
			query
				.columns([
					Alias::new("id"),
					Alias::new("title"),
					Alias::new("status"),
					Alias::new("revision"),
					Alias::new("dependencies"),
				])
				.expr_as(
					child_flags::open_or_claimed().and(child_flags::started()),
					Alias::new("started"),
				)
				.expr_as(
					child_flags::open_or_claimed().and(child_flags::abandonable(parent)),
					Alias::new("abandonable"),
				)
				.from(Alias::new(Self::table_name()))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.and_where(Expr::col("parent_id").eq(Expr::value(parent)))
				.order_by(Alias::new("id"), Order::Asc)
				.limit(100);
			if let Some(id) = after {
				query.and_where(Expr::col("id").gt(Expr::value(id)));
			}
			let (sql, values) = query.build(PostgresQueryBuilder);
			let rows = db.fetch_all(&sql, convert_values(values)).await?;
			let end = rows.len() < 100;
			for row in rows {
				let child: Child = serde_json::from_value(QueryRow::from_backend_row(row).data)?;
				after = Some(child.id);
				summary.include_task(child.id, child.status, child.revision);
				if !matches!(
					child.status,
					crate::domain::TaskStatus::Completed | crate::domain::TaskStatus::Abandoned
				) {
					pending.push(PendingChild {
						id: child.id,
						title: child.title,
						status: child.status,
						dependencies: child.dependencies,
						abandonable: child.abandonable,
						started: child.started,
					});
				}
			}
			if end {
				summary.unassigned = classify(&pending);
				return Ok(summary);
			}
		}
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

/// The message contract plus the exact File versions it cites, as a compact
/// record for the Run-message summary and workspace reads.
async fn message_record<E: OrmExecutor>(db: &mut E, row: Message) -> Result<Value> {
	let (sql, values) = Query::select()
		.columns(["file_id", "path", "digest"].map(Alias::new))
		.from(Alias::new("message_file_versions"))
		.and_where(Expr::col(Alias::new("message_id")).eq(Expr::value(row.id)))
		.build(PostgresQueryBuilder);
	let citations = db.fetch_all(&sql, convert_values(values)).await?;
	let mut value = json!(MessageContract::from(row));
	value["citations"] = json!(
		citations
			.into_iter()
			.map(|row| QueryRow::from_backend_row(row).data)
			.collect::<Vec<_>>()
	);
	Ok(value)
}
