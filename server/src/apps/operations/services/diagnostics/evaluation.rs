//! Read-only Reinhardt Query diagnostics for the fixed-task evaluator (#122).
//!
//! Usage Records have no HTTP read endpoint. The deterministic evaluation tier
//! selects complete Run/attempt inventories after whoever runs the Deployment
//! executes these parameterless queries inside the authorized Tenant's read-only RLS scope.
//! Cache identities are digests; prompts, salts, leases and raw context are excluded.
use reinhardt::query::{
	Alias, Expr, IntoIden, JoinType, Order, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SelectStatement, SimpleExpr, TableRef,
};

/// Every prompt-free Usage Record column, excluding leases. Estimation error is
/// `estimated_tokens - output_reservation - input_tokens`.
const USAGE_COLUMNS: &[&str] = &[
	"attempt_id",
	"run_id",
	"response_epoch",
	"model_id",
	"model_version",
	"projection_version",
	"harness_contract",
	"purpose",
	"requested_route",
	"provider_identity",
	"shadow_estimates",
	"cache_identity",
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
	"upstream_model",
	"byte_basis",
	"output_reservation",
	"estimate_ratio_ppm",
	"calibration_sample",
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
	json_rows(row.to_owned())
}

fn json_rows(row: SelectStatement) -> String {
	Query::select()
		.expr(SimpleExpr::FunctionCall(
			"row_to_json".into_iden(),
			vec![Expr::col(Alias::new("e")).into()],
		))
		.from_subquery(row, Alias::new("e"))
		.to_string(PostgresQueryBuilder)
}

fn decision_attempts() -> String {
	let mut row = Query::select();
	row.expr_as(Expr::col("id"), Alias::new("attempt_id"))
        .columns(["run_id", "decision_id", "step", "cycle", "retry", "route", "mode", "status", "decider", "created_at", "finished_at"].map(Alias::new))
        // The Decision transport reports no monetary usage. Retain each call,
        // explicitly unknown; neither its reservation nor owner receipts are money.
        .expr_as(Expr::cust("NULL::bigint"), Alias::new("cost_nanocredits"))
        .from(Alias::new("decision_attempts"))
        .order_by(Alias::new("created_at"), Order::Asc)
        .order_by(Alias::new("id"), Order::Asc);
	json_rows(row.to_owned())
}

fn memory_model_attempts() -> String {
	let mut row = Query::select();
	row.expr_as(Expr::col(("a", "id")), Alias::new("attempt_id"));
	for column in [
		"operation_id",
		"source_run_ids",
		"ordinal",
		"cycle",
		"state",
		"reserved_tokens",
		"reserved_cost_micros",
		"provider_usage",
		"created_at",
	] {
		row.column((Alias::new("a"), Alias::new(column)));
	}
	row.expr_as(Expr::col(("a", "tokens")), Alias::new("policy_tokens"))
		.expr_as(
			Expr::col(("a", "cost_micros")),
			Alias::new("policy_cost_micros"),
		)
		.expr_as(Expr::col(("o", "provider_id")), Alias::new("provider_id"))
		.expr_as(
			Expr::col(("o", "provider_version")),
			Alias::new("provider_version"),
		)
		.from_as(Alias::new("memory_model_attempts"), Alias::new("a"))
		.join(
			JoinType::InnerJoin,
			TableRef::table_alias(Alias::new("memory_model_operations"), Alias::new("o")),
			Expr::col(("a", "operation_id")).equals(("o", "id")),
		)
		.order_by(("a", "created_at"), Order::Asc)
		.order_by(("a", "id"), Order::Asc);
	json_rows(row.to_owned())
}

fn generation_embedding_usage() -> String {
	let mut row = Query::select();
	row.columns([
        "request_id", "attempt_id", "run_id", "workspace_id", "entry_id", "purpose",
        "provider_id", "provider_version", "request_bytes", "reserved_tokens", "reported_tokens", "created_at",
    ].map(Alias::new))
        // Ancestor budget reservations can describe the same physical memory
        // embedding. They establish unpriced work, never an additional charge.
        .expr_as(Expr::value("reservation"), Alias::new("accounting_kind"))
        .expr_as(Expr::value(true), Alias::new("provider_work_unpriced"))
        .expr_as(Expr::cust("NULL::bigint"), Alias::new("cost_nanocredits"))
        .from(Alias::new("generation_embedding_usage"))
        .order_by(Alias::new("created_at"), Order::Asc)
        .order_by(Alias::new("request_id"), Order::Asc)
        .order_by(Alias::new("attempt_id"), Order::Asc);
	json_rows(row.to_owned())
}

fn context_compaction_attempts() -> String {
	let mut row = Query::select();
	row.expr_as(Expr::col("id"), Alias::new("attempt_id"))
		.columns(
			[
				"run_id",
				"stage",
				"provider_dispatched",
				"policy_version",
				"provider",
				"source_from_seq",
				"source_through_seq",
				"base_revision",
				"observed_input_seq",
				"before_tokens",
				"outcome",
				"reason",
				"after_tokens",
				"created_at",
				"settled_at",
			]
			.map(Alias::new),
		)
		.from(Alias::new("context_compaction_attempts"))
		.order_by(Alias::new("created_at"), Order::Asc)
		.order_by(Alias::new("id"), Order::Asc);
	json_rows(row.to_owned())
}

fn run_recovery() -> String {
	let mut row = Query::select();
	row.expr_as(Expr::col("id"), Alias::new("run_id")).columns(
		[
			"task_id",
			"phase",
			"control",
			"step",
			"semantic_retry_cycle",
			"updated_at",
		]
		.map(Alias::new),
	);
	// Fixed JSON paths project typed counters only, never a context or error body.
	for (expression, name) in [
		(
			"COALESCE((context->>'compactions')::bigint, 0)",
			"compactions",
		),
		(
			"COALESCE((context->'recovery'->>'overflow_attempts')::integer, 0)",
			"overflow_attempts",
		),
		(
			"(context->'recovery'->>'overflow_request_tokens')::bigint",
			"overflow_request_tokens",
		),
		(
			"COALESCE((context->'recovery'->>'window_permille')::integer, 1000)",
			"window_permille",
		),
		(
			"COALESCE((context->'estimate'->>'demoted')::boolean, false)",
			"estimator_demoted",
		),
		("pending->'recovery'->>'context_reason'", "context_reason"),
	] {
		row.expr_as(Expr::cust(expression), Alias::new(name));
	}
	row.from(Alias::new("runs"))
		.order_by(Alias::new("id"), Order::Asc);
	json_rows(row.to_owned())
}

pub(super) fn document() -> serde_json::Value {
	serde_json::json!({
		"inference_usage": inference_usage(),
		"decision_attempts": decision_attempts(),
		"memory_model_attempts": memory_model_attempts(),
		"generation_embedding_usage": generation_embedding_usage(),
		"context_compaction_attempts": context_compaction_attempts(),
		"run_recovery": run_recovery(),
	})
}
