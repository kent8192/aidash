//! Current lifecycle authority and exact saved settlement evidence for Home commands.
use crate::{Error, Result};
use aidash_domain::{RunControl, RunMetadata, run_state::RawRun};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;

/// Composed into the existing admitted_sender transaction; no nested pool or RPC.
pub(crate) async fn require_in(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	cached: &RunMetadata,
	operation: &str,
	data: &Value,
) -> Result<()> {
	let raw: RawRun = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(cached.id)))
			.and_where(Expr::col("tenant").eq(tenant))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
	.await?;
	if raw.metadata.node != cached.node
		|| raw.metadata.home_node != cached.home_node
		|| raw.metadata.task_id != cached.task_id
		|| raw.metadata.workspace_id != cached.workspace_id
	{
		return Err(Error::Forbidden);
	}
	let admission: (bool, Value) = crate::database::native::query_as(
		&Query::select()
			.columns([Alias::new("cancelled"), Alias::new("description")])
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::col("id").eq(Expr::value(cached.id)))
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("node").eq(cached.node.as_str()))
			.and_where(Expr::col("source_node").eq(cached.home_node.as_str()))
			.to_string(PostgresQueryBuilder),
	)
	.columns(&["cancelled", "description"])
	.fetch_one(&mut *tx)
	.await?;
	let captured: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("node_cancellation_jobs"))
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("kind").is_in(["run", "admission"]))
			.and_where(Expr::col("work_id").eq(Expr::value(cached.id)))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	if !admission.0
		&& captured.is_none()
		&& raw.metadata.control == RunControl::Active
		&& !raw.metadata.phase.is_terminal()
	{
		return Ok(());
	}
	let inputs: Vec<aidash_domain::run_input::RunInput> = crate::database::query_as(
		&Query::select()
			.column(reinhardt::query::ColumnRef::Asterisk)
			.from(Alias::new("run_inputs"))
			.and_where(Expr::col("run_id").eq(Expr::value(cached.id)))
			.and_where(Expr::col("tenant").eq(tenant))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?;
	let description: aidash_domain::federation::execution::Description =
		serde_json::from_value(admission.1)?;
	if aidash_domain::federation::execution::settlement::allows(
		&raw,
		&inputs,
		&aidash_application::execution::result_artifact_name(&description.task.title),
		operation,
		data,
	) {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
