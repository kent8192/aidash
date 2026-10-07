//! Natural retirement drains opted-in learning without minting a post-Run budget.
use super::{process, schedule_run};
use crate::{Result, database::native, store::Store};
use aidash_domain::{Run, generation::requests::Request};
use reinhardt::query::types::PgBinOper;
use reinhardt::query::{
	Alias, BinOper, ColumnRef, Condition, Expr, ExprTrait, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};

pub(crate) async fn completion_ready(store: &Store, job: &Request) -> Result<bool> {
	let Some(run): Option<Run> = crate::database::query_as(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("home_node").eq(store.node_id.as_str()))
			.and_where(Expr::col("task_id").eq(Expr::value(job.task_id)))
			.and_where(Expr::col("agent_id").eq(job.agent_id.as_str()))
			.and_where(Expr::col("agent_version").eq(job.agent_version.as_str()))
			.and_where(Expr::col("phase").eq("COMPLETED"))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&store.pool)
	.await?
	else {
		return Ok(true);
	};
	let Some(bank) = schedule_run(store, run.id).await? else {
		return Ok(true);
	};
	// A persisted claim arbitrates with the independent janitor. Provider I/O
	// holds neither the retirement transaction nor the generation request lock.
	process(store, run.id).await?;
	let state: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("memory_engine_jobs"))
			.and_where(Expr::col("id").eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await?;
	if state != "complete" {
		return Ok(!matches!(state.as_str(), "pending" | "running"));
	}
	// Human review and its admitted-unit indexing use the same reserved origin
	// while it is live. The original expiry and explicit stop/delete controls
	// remain hard limits; waiting never allocates another call or extends TTL.
	for (table, states) in [
		("memory_candidates", vec!["pending"]),
		("memory_engine_jobs", vec!["pending", "running"]),
	] {
		let origin = if table == "memory_candidates" {
			Condition::all().add(
				Expr::col("run")
					.binary(
						BinOper::PgOperator(PgBinOper::JsonGetAsText),
						Expr::value("id"),
					)
					.eq(run.id.to_string()),
			)
		} else {
			let units = || {
				Query::select()
					.expr(Expr::col("unit_id").cast_as("text"))
					.from(Alias::new("memory_unit_run_origins"))
					.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
					.to_owned()
			};
			Condition::any()
				.add(Expr::col("id").eq(Expr::value(run.id)))
				.add(
					Expr::col("input")
						.binary(
							BinOper::PgOperator(PgBinOper::JsonGetPathAsText),
							Expr::value("{source,id}"),
						)
						.binary(BinOper::In, SimpleExpr::SubQuery(None, Box::new(units()))),
				)
				.add(
					Expr::col("input")
						.binary(
							BinOper::PgOperator(PgBinOper::JsonGetAsText),
							Expr::value("target"),
						)
						.binary(BinOper::In, SimpleExpr::SubQuery(None, Box::new(units()))),
				)
				.add(
					Expr::col("input")
						.binary(
							BinOper::PgOperator(PgBinOper::JsonGetAsText),
							Expr::value("origin_run"),
						)
						.eq(run.id.to_string()),
				)
		};
		if native::query(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new(table))
				.and_where(Expr::col("bank_id").eq(Expr::value(bank)))
				.and_where(Expr::col("state").is_in(states))
				.cond_where(origin)
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&store.pool)
		.await?
		.is_some()
		{
			return Ok(false);
		}
	}
	let units = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("memory_units"))
		.and_where(Expr::col("bank_id").eq(Expr::value(bank)))
		.and_where(Expr::col("deleted").eq(Expr::value(false)))
		.and_where(
			Expr::col("id").in_subquery(
				Query::select()
					.column(Alias::new("unit_id"))
					.from(Alias::new("memory_unit_run_origins"))
					.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
					.to_owned(),
			),
		)
		.to_owned();
	Ok(native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("semantic_entries"))
			.and_where(Expr::col("id").in_subquery(units))
			.and_where(Expr::col("state").is_in(["PENDING", "ERROR"]))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&store.pool)
	.await?
	.is_none())
}
