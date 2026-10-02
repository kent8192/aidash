//! Read-only SeaQuery diagnostics for the remote-memory cluster fixtures.
use sea_orm::sea_query::{Alias, Expr, Func, PostgresQueryBuilder, Query, SimpleExpr};

fn rows(table: &str, columns: &[&str], predicate: &str) -> String {
	rows_where(table, columns, Expr::cust(predicate))
}

fn rows_where(table: &str, columns: &[&str], predicate: SimpleExpr) -> String {
	let mut row = Query::select();
	row.columns(columns.iter().map(|name| Alias::new(*name)))
		.from(Alias::new(table))
		.and_where(predicate);
	Query::select()
		.expr(Func::cust(Alias::new("row_to_json")).arg(Expr::col(Alias::new("e"))))
		.from_subquery(row.to_owned(), Alias::new("e"))
		.to_string(PostgresQueryBuilder)
}

fn main() {
	let attempts = Expr::col(Alias::new("operation_id")).in_subquery(
		Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("semantic_remote_operations"))
			.and_where(Expr::cust("admission_id=:'run'::uuid"))
			.to_owned(),
	);
	println!(
		"{}",
		serde_json::json!({
			"runs": rows("runs", &["id", "task_id", "workspace_id", "agent_id", "agent_version", "phase", "control", "pending", "lease_until"], "task_id=:'task'::uuid"),
			"origins": rows("authorization_task_origins", &["task_id", "source_run_id", "subject_chain"], "task_id=:'task'::uuid"),
			"receipts": rows("semantic_remote_receipts", &["operation_id", "run_id", "digest", "receipt"], "run_id=:'run'::uuid"),
			"operations": rows("semantic_remote_operations", &["id", "state", "cycle", "failures", "attempt_id", "fence", "error"], "admission_id=:'run'::uuid"),
			"attempts": rows_where("semantic_remote_attempts", &["id", "operation_id", "fence", "state"], attempts),
			"usage": rows("generation_remote_usage", &["request_id", "attempt_id", "operation_id", "purpose", "state", "reserved_tokens", "reported_tokens"], "admission_id=:'run'::uuid"),
			"dispatches": rows("generation_remote_dispatches", &["attempt_id", "usage", "state", "peer_finalized"], "usage->>'admission_id'=:'run'"),
			"requests": rows("generation_requests", &["id", "home_node", "task_id", "agent_id", "agent_version", "status", "admission_id"], "task_id=:'task'::uuid"),
			"budgets": rows("generation_budgets", &["request_id", "used_tokens", "embedding_calls", "compaction_calls"], "TRUE")
		})
	);
}
