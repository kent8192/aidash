//! Read-only Reinhardt Query diagnostics for the fixed-task evaluator (#122).
//!
//! Usage Records have no HTTP read endpoint. The deterministic evaluation tier
//! owns a disposable database and reads its Usage Records with this query,
//! then selects one Run's records by `run_id` itself, so the query has no
//! parameter. No prompt text is selected.
use reinhardt::query::{
	Alias, Expr, IntoIden, Order, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};

/// Every Usage Record column the evaluator reports, excluding the lease token
/// and the reserved cache identity.
const USAGE_COLUMNS: &[&str] = &[
	"attempt_id",
	"run_id",
	"response_epoch",
	"model_id",
	"model_version",
	"projection_version",
	"input_tokens",
	"output_tokens",
	"cache_read_tokens",
	"cache_write_tokens",
	"reasoning_tokens",
	"cost_nanocredits",
	"upstream_cost_nanocredits",
	"estimated_tokens",
	"estimator",
	"estimator_version",
	"estimate_confidence",
	"outcome",
	"rejection_class",
	"created_at",
	"completed_at",
];

pub(super) fn inference_usage() -> String {
	let mut row = Query::select();
	row.columns(USAGE_COLUMNS.iter().map(|name| Alias::new(*name)))
		.from(Alias::new("inference_usage"))
		.order_by(Alias::new("created_at"), Order::Asc)
		.order_by(Alias::new("attempt_id"), Order::Asc);
	Query::select()
		.expr(SimpleExpr::FunctionCall(
			"row_to_json".into_iden(),
			vec![Expr::col(Alias::new("e")).into()],
		))
		.from_subquery(row.to_owned(), Alias::new("e"))
		.to_string(PostgresQueryBuilder)
}

pub(super) fn document() -> serde_json::Value {
	serde_json::json!({ "inference_usage": inference_usage() })
}
