use super::*;

/// The Decider call count leaves out only the unanswered attempts of an outage
/// retry (ADR 0129). The predicate is grouped beside the count's other
/// conditions, and its labels are the stored `AttemptStatus` labels.
#[test]
fn the_call_count_leaves_out_only_unanswered_outage_retries() {
	let sql = Query::select()
		.expr(Expr::cust("COUNT(*)"))
		.from(Alias::new(ATTEMPTS))
		.and_where(Expr::col("run_id").eq(Expr::value(Uuid::nil())))
		.and_where(counted_attempt().unwrap())
		.to_string(PostgresQueryBuilder);
	let labels = [
		AttemptStatus::Failed,
		AttemptStatus::Uncertain,
		AttemptStatus::NotDispatched,
	]
	.map(|status| format!("'{}'", status_label(status).unwrap()))
	.join(", ");
	assert_eq!(labels, "'failed', 'uncertain', 'not_dispatched'");
	assert!(
		sql.ends_with(&format!(
			r#" AND ("retry" <= 0 OR "status" IS NULL OR "status" NOT IN ({labels}))"#
		)),
		"{sql}"
	);
}
