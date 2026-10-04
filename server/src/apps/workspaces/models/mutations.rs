//! Selective writes on the caller's native transaction.
use super::{Artifact, Task, Workspace};
use crate::apps::workspaces::serializers::entities::{
	Artifact as ArtifactContract, ArtifactInput, Task as TaskContract,
	Workspace as WorkspaceContract,
};
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;
use uuid::Uuid;

impl Workspace {
	pub(crate) async fn lock_existing(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<()> {
		let (sql, values) = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Update)
			.build(PostgresQueryBuilder);
		if tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_none()
		{
			return Err(Error::NotFound("workspace".into()));
		}
		Ok(())
	}

	/// The use case validates the state contract before attempting this CAS.
	pub(crate) async fn replace_state(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
		revision: i64,
		state: Value,
	) -> Result<WorkspaceContract> {
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("state"), Expr::value(state))
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(reinhardt::query::Expr::value(1_i64)),
			)
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("revision").eq(Expr::value(revision)))
			.returning_all()
			.build(PostgresQueryBuilder);
		let row = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.ok_or_else(|| Error::Conflict("workspace revision changed".into()))?;
		Ok(serde_json::from_value(
			QueryRow::from_backend_row(row).data,
		)?)
	}
}

impl Task {
	pub(crate) async fn lock(tx: &mut dyn TransactionExecutor, id: Uuid) -> Result<Self> {
		Self::objects()
			.filter(Self::field_id().eq(id))
			.select_for_update()
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?
			.pop()
			.ok_or_else(|| Error::Conflict("task unavailable".into()))
	}

	pub(crate) async fn unfinished_children(
		tx: &mut dyn TransactionExecutor,
		id: Uuid,
	) -> Result<bool> {
		let children = Query::select()
			.expr(Expr::value(1_i64))
			.from(Alias::new(Self::table_name()))
			.and_where(Expr::col("parent_id").eq(Expr::value(id)))
			.and_where(Expr::col("status").is_not_in(["COMPLETED", "ABANDONED"]))
			.to_owned();
		let (sql, values) = Query::select()
			.expr_as(Expr::exists(children), Alias::new("pending"))
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		Ok(row.get("pending").map_err(FrameworkError::from)?)
	}

	/// The caller holds the task lock and has checked its expected revision.
	pub(crate) async fn complete(
		tx: &mut dyn TransactionExecutor,
		task: &Self,
		artifact: &ArtifactInput,
		key: &str,
	) -> Result<(TaskContract, ArtifactContract)> {
		let draft = Artifact::build()
			.workspace_id(task.workspace_id)
			.task_id(task.id)
			.kind(serde_json::from_value(Value::String(
				artifact.kind.clone(),
			))?)
			.name(&artifact.name)
			.content(artifact.content.clone().into())
			.created_by(
				task.owner
					.as_deref()
					.ok_or_else(|| Error::Conflict("task has no owner".into()))?,
			)
			.idempotency_key(key)
			.finish();
		let artifact = Artifact::objects()
			.insert_with_executor(tx, &draft)
			.await
			.map_err(FrameworkError::from)?;
		// Do not rewrite topology columns: graph writers take a separate lock.
		let (sql, values) = Query::update()
			.table(Alias::new(Self::table_name()))
			.value_expr(Alias::new("status"), Expr::value("COMPLETED"))
			.value_expr(Alias::new("completion_key"), Expr::value(key))
			.value_expr(
				Alias::new("revision"),
				Expr::col("revision").add(reinhardt::query::Expr::value(1_i64)),
			)
			.and_where(Expr::col("id").eq(Expr::value(task.id)))
			.returning_all()
			.build(PostgresQueryBuilder);
		let row = tx.fetch_one(&sql, convert_values(values)).await?;
		Ok((
			serde_json::from_value(QueryRow::from_backend_row(row).data)?,
			artifact.into(),
		))
	}
}
