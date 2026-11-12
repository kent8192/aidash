//! Journal copies a Run holds of another Run's Research (#850). A Journal
//! read across Runs keeps the source Runs in its dependencies, so the source
//! Run's Evidence Revocation and account-terms expiry withdraw the copy
//! wherever it is stored: the holder's invocation, its `tool.completed` and
//! `tool.started` events, and its frozen Research Journal variant.
use super::*;
use aidash_domain::context::research::{JOURNAL_QUERY_CONTRACT, JOURNAL_RETRIEVAL_CONTRACT};
use aidash_domain::research::expiry::ExpiredResults;
use std::collections::BTreeMap;

/// Which source Runs a sweep withdraws copies of.
#[derive(Clone, Copy)]
enum Sources {
	One(Uuid),
	/// Every Run whose evidence was revoked.
	Revoked,
}

/// `json` names a source Run: `json @> {"dependencies":{"runs":[run]}}` for a
/// stored result, or `json @> {"runs":[run]}` for a stored reference.
fn names(json: SimpleExpr, sources: Sources, nested: bool) -> SimpleExpr {
	let contains = |value: SimpleExpr| -> SimpleExpr {
		Expr::from(json.clone()).binary(
			reinhardt::query::types::BinOper::PgOperator(
				reinhardt::query::types::PgBinOper::Contains,
			),
			value,
		)
	};
	match sources {
		Sources::One(run) => contains(Expr::value(if nested {
			serde_json::json!({"dependencies":{"runs":[run]}})
		} else {
			serde_json::json!({"runs":[run]})
		})),
		Sources::Revoked => {
			let object = |key: &str, value: SimpleExpr| -> SimpleExpr {
				SimpleExpr::FunctionCall(
					"jsonb_build_object".into_iden(),
					vec![Expr::value(key).into(), value],
				)
			};
			let runs = object(
				"runs",
				SimpleExpr::FunctionCall(
					"jsonb_build_array".into_iden(),
					vec![
						Expr::col((
							Alias::new(ResearchRevocation::table_name()),
							Alias::new("run_id"),
						))
						.cast_as_text()
						.into(),
					],
				),
			);
			let value = if nested {
				object("dependencies", runs)
			} else {
				runs
			};
			Expr::exists(
				Query::select()
					.expr(Expr::value(1))
					.from(Alias::new(ResearchRevocation::table_name()))
					.and_where(contains(value))
					.to_owned(),
			)
			.into()
		}
	}
}

/// A research-derived Journal result.
fn journal_copy(result: SimpleExpr) -> SimpleExpr {
	json_text(Expr::from(result.clone()), "contract")
		.is_in([JOURNAL_QUERY_CONTRACT, JOURNAL_RETRIEVAL_CONTRACT])
		.and(json_text(Expr::from(result), "research_derived").eq("true"))
}

/// The holders' invocations of research-derived Journal copies naming
/// `sources`, with their results, by holder Run.
async fn copies(
	tx: &mut dyn TransactionExecutor,
	sources: Sources,
) -> Result<BTreeMap<Uuid, Vec<(String, Value)>>> {
	let result = || SimpleExpr::from(Expr::col(Alias::new("result")));
	let mut select = Query::select();
	select
		.columns(["run_id", "idempotency_key", "result"].map(Alias::new))
		.from(Alias::new("invocations"))
		.and_where(journal_copy(result()))
		.and_where(names(result(), sources, true));
	if let Sources::One(run) = sources {
		select.and_where(Expr::col("run_id").ne(Expr::value(run)));
	}
	let (sql, values) = select.build(PostgresQueryBuilder);
	let mut found: BTreeMap<Uuid, Vec<(String, Value)>> = BTreeMap::new();
	for row in tx.fetch_all(&sql, convert_values(values)).await? {
		let data = QueryRow::from_backend_row(row).data;
		found
			.entry(serde_json::from_value(data["run_id"].clone())?)
			.or_default()
			.push((
				serde_json::from_value(data["idempotency_key"].clone())?,
				data["result"].clone(),
			));
	}
	Ok(found)
}

/// Replace the holder's copies `keys` with `tombstone` in its invocations and
/// `tool.completed` events, and with `redact` also their disclosed inputs.
async fn replace(
	tx: &mut dyn TransactionExecutor,
	holder: Uuid,
	keys: &[String],
	tombstone: &Value,
	redact: bool,
) -> Result<u64> {
	let keyed = || keys.iter().cloned().map(Expr::value);
	let mut update = Query::update();
	update
		.table(Alias::new("invocations"))
		.value(Alias::new("result"), tombstone.clone())
		.and_where(Expr::col("run_id").eq(Expr::value(holder)))
		.and_where(Expr::col("idempotency_key").is_in(keyed()));
	if redact {
		update.value(Alias::new("input"), revoked_arguments());
	}
	let (sql, values) = update.build(PostgresQueryBuilder);
	let replaced = tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected;
	let data = || Expr::col(Alias::new("data"));
	let mut events = vec![("tool.completed", "result", tombstone.to_string())];
	if redact {
		events.push(("tool.started", "input", revoked_arguments().to_string()));
	}
	for (kind, key, value) in events {
		let (sql, values) = Query::update()
			.table(Alias::new("events"))
			.value_expr(Alias::new("data"), with_key(data(), key, &value))
			.and_where(Expr::col("kind").eq(kind))
			.and_where(json_text(data(), "run_id").eq(Expr::value(holder.to_string())))
			.and_where(json_text(data(), "idempotency_key").is_in(keyed()))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	Ok(replaced)
}

/// Tombstone, in every other Run, the Journal copies naming `run` as a
/// source, or with none those naming any revoked Run, which catches a copy
/// stored just after its source was revoked. Idempotent.
pub(crate) async fn revoke_copies(
	tx: &mut dyn TransactionExecutor,
	run: Option<Uuid>,
) -> Result<u64> {
	let sources = run.map_or(Sources::Revoked, Sources::One);
	let tombstone = revoked_result(Operation::Search);
	let mut removed = 0;
	for (holder, found) in copies(tx, sources).await? {
		let keys: Vec<_> = found.into_iter().map(|(key, _)| key).collect();
		removed += replace(tx, holder, &keys, &tombstone, true).await?;
	}
	// A copy whose event outlived its invocation result.
	let data = || Expr::col(Alias::new("data"));
	let result = || json_field(data(), "result");
	let mut events = Query::update();
	events
		.table(Alias::new("events"))
		.value_expr(
			Alias::new("data"),
			with_key(data(), "result", &tombstone.to_string()),
		)
		.and_where(Expr::col("kind").eq("tool.completed"))
		.and_where(journal_copy(result()))
		.and_where(names(result(), sources, true));
	let (sql, values) = events.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let (sql, values) = Query::update()
		.table(Alias::new(ResearchJournalResult::table_name()))
		.value(Alias::new("bytes"), Option::<Vec<u8>>::None)
		.value(Alias::new("purge_reason"), "revocation")
		.and_where(names(
			Expr::col(Alias::new("reference")).into(),
			sources,
			false,
		))
		.and_where(
			Expr::col("purge_reason")
				.is_null()
				.or(Expr::col("purge_reason").ne("revocation")),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(removed)
}

/// Tombstone, in every other Run, the Journal copies of `run`'s Research
/// that `expired` covers, as the same Run's own copies are (ADR 0058).
pub(crate) async fn expire_copies(
	tx: &mut dyn TransactionExecutor,
	run: Uuid,
	expired: &ExpiredResults,
) -> Result<()> {
	if expired.is_empty() {
		return Ok(());
	}
	let tombstone = aidash_domain::research::expiry::tombstone(Operation::Search);
	for (holder, found) in copies(tx, Sources::One(run)).await? {
		let keys: Vec<_> = found
			.into_iter()
			.filter(|(_, result)| expired.covers(result))
			.map(|(key, _)| key)
			.collect();
		if !keys.is_empty() {
			replace(tx, holder, &keys, &tombstone, false).await?;
		}
	}
	ResearchJournalResult::expire_copies(tx, run, expired).await
}
