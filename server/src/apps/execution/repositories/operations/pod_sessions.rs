//! Durable evidence that a Task execution may still have an interpreter or MCP process.
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::{
	capabilities::pod::PYTHON,
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use reinhardt::query::{
	Alias, BinOper, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, types::PgBinOper,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);
const TERMINAL: [&str; 3] = ["COMPLETED", "FAILED", "CANCELLED"];

#[derive(Serialize, Deserialize)]
pub(super) struct StdioSession {
	pub(super) tenant: String,
	pub(super) credential: Uuid,
	pub(super) subject: String,
	pub(super) subjects: Vec<String>,
	pub(super) space: String,
}

impl StdioSession {
	pub(super) fn identity(&self) -> SubjectIdentity {
		SubjectIdentity {
			http_session: None,
			credential_id: self.credential,
			tenant: self.tenant.clone(),
			subject: self.subject.clone(),
		}
	}
}

/// Register before handing the caller a Pod context. A lost start response or
/// a worker restart leaves the Task execution visible to the Read Recheck.
pub(crate) async fn register_stdio(access: &mut Access, run: Uuid, space: &str) -> Result<()> {
	let evidence = StdioSession {
		tenant: access.identity.tenant.clone(),
		credential: access.identity.credential_id,
		subject: access.identity.subject.clone(),
		subjects: access.subjects.clone(),
		space: space.to_owned(),
	};
	let written = crate::database::native::query(
		&Query::update()
			.table(Alias::new("task_executions"))
			.value(
				Alias::new("stdio_session"),
				serde_json::to_value(&evidence)?,
			)
			.and_where(Expr::col("id").eq(run))
			.and_where(Expr::col("tenant").eq(evidence.tenant))
			.and_where(Expr::col("phase").is_not_in(TERMINAL))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **access.tx)
	.await?;
	if written.rows_affected() == 0 {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub(super) async fn stdio(store: &Store, run: Uuid) -> Result<Option<StdioSession>> {
	let session: Option<serde_json::Value> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("stdio_session"))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("id").eq(run))
			.and_where(Expr::col("stdio_session").is_not_null())
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(store.pool.in_scope(&MAINTENANCE))
	.await?;
	session
		.map(serde_json::from_value)
		.transpose()
		.map_err(Into::into)
}

pub(super) async fn stdio_targets(store: &Store) -> Result<Vec<Uuid>> {
	crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("stdio_session").is_not_null())
			.and_where(super::read_recheck::unrevoked("task_executions", "id"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(store.pool.in_scope(&MAINTENANCE))
	.await
}

pub(super) async fn python_targets(store: &Store) -> Result<Vec<Uuid>> {
	crate::database::native::query_scalar(
		&Query::select()
			.distinct_on([Alias::new("run_id")])
			.column(Alias::new("id"))
			.from(Alias::new("core_operations"))
			.and_where(Expr::col("kind").eq(PYTHON))
			.and_where(Expr::col("state").is_not_in(["prepared", "submitted", "running"]))
			.and_where(
				Expr::col("input")
					.binary(
						BinOper::PgOperator(PgBinOper::JsonGetAsText),
						Expr::value("runtime"),
					)
					.eq("pod"),
			)
			.and_where(
				Expr::col("result")
					.binary(
						BinOper::PgOperator(PgBinOper::Contains),
						Expr::value(json!({"sessions_stopped": true})),
					)
					.not(),
			)
			.and_where(super::read_recheck::unrevoked("core_operations", "run_id"))
			.order_by(Alias::new("run_id"), Order::Asc)
			.order_by(Alias::new("updated_at"), Order::Desc)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(store.pool.in_scope(&MAINTENANCE))
	.await
}

/// Try the idempotent Runner stop only after the terminal phase is durable.
/// An unconfirmed stop leaves the targets intact for the next pass or process.
pub(super) async fn stop_if_ended(
	store: &Store,
	tenant: &str,
	run: Uuid,
	space: &str,
) -> Result<()> {
	let scope = TenancyScope::tenant(tenant)?;
	let terminal: Option<Uuid> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(run))
			.and_where(Expr::col("phase").is_in(TERMINAL))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(store.pool.in_scope(&scope))
	.await?;
	if terminal.is_none() {
		return Ok(());
	}
	let runner = crate::bootstrap::operation_runner(store)?;
	runner
		.stop_sessions(space, &run.to_string())
		.await
		.map_err(|error| Error::External(error.to_string()))?;

	let mut tx = store.pool.begin(&scope).await?;
	crate::database::native::query(
		&Query::update()
			.table(Alias::new("task_executions"))
			.value_expr(
				Alias::new("stdio_session"),
				Expr::value(Option::<Value>::None),
			)
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("id").eq(run))
			.and_where(
				Expr::col("stdio_session")
					.binary(
						BinOper::PgOperator(PgBinOper::JsonGetAsText),
						Expr::value("space"),
					)
					.eq(space.to_owned()),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	crate::database::native::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value_expr(
				Alias::new("result"),
				Expr::col("result").binary(
					BinOper::PgOperator(PgBinOper::Concatenate),
					Expr::value(json!({"sessions_stopped": true})),
				),
			)
			.and_where(Expr::col("tenant").eq(tenant.to_owned()))
			.and_where(Expr::col("run_id").eq(run))
			.and_where(Expr::col("kind").eq(PYTHON))
			.and_where(
				Expr::col("input")
					.binary(
						BinOper::PgOperator(PgBinOper::JsonGetAsText),
						Expr::value("runtime"),
					)
					.eq("pod"),
			)
			.and_where(
				Expr::col("input")
					.binary(
						BinOper::PgOperator(PgBinOper::JsonGetAsText),
						Expr::value("scratch_space_id"),
					)
					.eq(space.to_owned()),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(())
}
