//! Safety is affirmative: terminal status alone is not physical writer stop.
use super::*;
use reinhardt::query::SimpleExpr;
use selectors::{col, json};

async fn refuse(
	tx: &mut dyn TransactionExecutor,
	table: &str,
	predicate: SimpleExpr,
	reason: &str,
) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(Expr::value(1_i32))
		.from(Alias::new(table))
		.and_where(predicate)
		.limit(1)
		.build(PostgresQueryBuilder);
	if tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.is_some()
	{
		return Err(Error::Conflict(format!("Node purge {reason}")));
	}
	Ok(())
}
pub(super) async fn check(
	tx: &mut dyn TransactionExecutor,
	owners: &Owners<'_>,
	snapshot: &Snapshot,
) -> Result<()> {
	// Every durable job fences erasure, including jobs reserved for a future retry.
	refuse(
		tx,
		"node_cancellation_jobs",
		owners.predicate("node_cancellation_jobs")?,
		"awaits durable cancellation settlement",
	)
	.await?;
	let mut policy = snapshot.bundle.clone();
	policy
		.subjects
		.retain(|name, _| !owners.subjects.contains(name));
	if policy.validate().is_err() {
		return Err(Error::Conflict("Node purge blocked by surviving policy delegation; repair the delegating Subject before retrying".into()));
	}
	// Atomic journals have no verified Node owner. A conservative global barrier
	// may delay an unrelated Node, but never destroys decided recovery anchors.
	refuse(
		tx,
		"atomic_coordinators",
		Expr::col("complete").eq(Expr::value(false)),
		"awaits incomplete atomic recovery",
	)
	.await?;
	refuse(
		tx,
		"atomic_participants",
		Expr::col("phase").is_not_in(["COMMITTED", "ABORTED"]),
		"awaits nonterminal atomic participant recovery",
	)
	.await?;
	let now = database_time(tx).await?;
	let area_runs = Query::select()
		.column(Alias::new("run_id"))
		.from(Alias::new("core_runs"))
		.and_where(Expr::col("tenant").eq(Expr::value(owners.tenant)))
		.and_where(owners.root("core_runs", "area_id", classification::node::Root::Area)?)
		.to_owned();
	refuse(
		tx,
		"runs",
		owners
			.predicate("runs")?
			.or(Expr::col("id").in_subquery(area_runs))
			.and(
				Expr::col("phase")
					.is_not_in(["COMPLETED", "FAILED", "CANCELLED"])
					.and(Expr::col("control").ne(Expr::value("CANCELLED")))
					.or(Expr::col("lease_until").gt(Expr::value(now))),
			),
		"awaits Run cancellation and worker lease drain",
	)
	.await?;
	refuse(
		tx,
		"generation_requests",
		owners
			.predicate("generation_requests")?
			.and(Expr::col("quota_released").eq(Expr::value(false))),
		"awaits generation quota settlement",
	)
	.await?;
	refuse(
		tx,
		"generation_remote_usage",
		owners
			.predicate("generation_remote_usage")?
			.and(Expr::col("state").eq(Expr::value("RESERVED"))),
		"awaits remote generation settlement",
	)
	.await?;
	refuse(
		tx,
		"generation_remote_dispatches",
		owners.predicate("generation_remote_dispatches")?.and(
			Expr::col("state")
				.is_not_in(["ABORTED", "SETTLED"])
				.or(Expr::col("peer_finalized").ne(Expr::value(true))),
		),
		"awaits peer generation finalization",
	)
	.await?;
	refuse(
		tx,
		"generation_remote_finalizations",
		owners
			.predicate("generation_remote_finalizations")?
			.and(Expr::col("result").is_null()),
		"awaits durable generation finalization",
	)
	.await?;
	refuse(
		tx,
		"semantic_remote_operations",
		owners
			.predicate("semantic_remote_operations")?
			.and(Expr::col("lease_until").gt(Expr::value(now))),
		"awaits the semantic journal worker lease drain",
	)
	.await?;
	refuse(
		tx,
		"semantic_remote_attempts",
		owners
			.predicate("semantic_remote_attempts")?
			.and(Expr::col("state").is_in(["RESERVING", "DISPATCHED"])),
		"awaits semantic attempt settlement",
	)
	.await?;
	let terminal = Expr::col("state").is_in(["completed", "cancelled", "failed", "withdrawn"]);
	let proof = |field: &str, value: bool| {
		col("core_operations", "result")
			.binary(
				reinhardt::query::BinOper::PgOperator(
					reinhardt::query::types::PgBinOper::JsonGetByIndex,
				),
				Expr::value(field).into(),
			)
			.eq(Expr::value(serde_json::json!(value)))
	};
	let stopped = proof("termination_confirmed", true).and(proof("runner_acknowledged", true));
	let never = Expr::col("runner_instance")
		.is_null()
		.and(proof("effects_may_have_occurred", false));
	let safe = terminal
		.and(stopped.clone().or(never))
		.or(Expr::col("state").eq(Expr::value("uncertain")).and(stopped));
	refuse(
		tx,
		"core_operations",
		owners
			.predicate("core_operations")?
			.or(owners.root("core_operations", "run_id", classification::node::Root::Run)?)
			.and(selectors::not_true(safe)),
		"awaits confirmed command termination, output drain and runner acknowledgement",
	)
	.await?;
	let python = Expr::col("kind")
		.eq(Expr::value("python_session"))
		.and(Expr::col("state").is_in(["initial", "running", "frozen"]));
	let extractor = Expr::col("kind")
		.eq(Expr::value("reference"))
		.and(json(col("core_records", "data"), "instance").is_not_null())
		.and(json(col("core_records", "data"), "operation_id").is_not_null())
		.and(selectors::not_true(
			json(col("core_records", "data"), "runner_released").eq(Expr::value("true")),
		));
	let outbound = Expr::col("kind")
		.eq(Expr::value("outbound"))
		.and(Expr::col("state").eq(Expr::value("attempted")));
	let transfer = Expr::col("kind")
		.eq(Expr::value("transfer_out"))
		.and(Expr::col("state").is_in(["transferring", "committing"]));
	refuse(
		tx,
		"core_records",
		owners
			.predicate("core_records")?
			.and(python.or(extractor).or(outbound).or(transfer)),
		"awaits Python/extractor/outbound writer shutdown or transfer settlement",
	)
	.await?;
	Ok(())
}
