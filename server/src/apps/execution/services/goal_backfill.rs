//! Reinhardt Query output for the Goal and Completion Message cutover.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

const SCOPE: &str = "SET LOCAL aidash.scope = 'deployment';\n";

pub(crate) fn execution() -> String {
	let registry = Query::update()
		.table(Alias::new("registry"))
		.value_expr(
			Alias::new("metadata"),
			Expr::cust("jsonb_set(metadata, '{config}', (metadata->'config') - 'completion')"),
		)
		.and_where(Expr::col("kind").eq("agent"))
		.and_where(Expr::cust("metadata->'config' ? 'completion'"))
		.to_string(PostgresQueryBuilder);
	let context = Query::update()
		.table(Alias::new("task_executions"))
		.value_expr(Alias::new("context"), Expr::cust("context - 'completion'"))
		.and_where(Expr::cust("context ? 'completion'"))
		.to_string(PostgresQueryBuilder);
	let summary = Query::update()
		.table(Alias::new("task_executions"))
		.value_expr(
			Alias::new("context"),
			Expr::cust(
				"jsonb_set(context, '{execution_summary,content}', \
			 (context #> '{execution_summary,content}') - 'goal' || \
			 jsonb_build_object('intent', context #> '{execution_summary,content,goal}'))",
			),
		)
		.and_where(Expr::cust(
			"context #> '{execution_summary,content,goal}' IS NOT NULL",
		))
		.to_string(PostgresQueryBuilder);
	let reads = Query::delete()
		.from_table(Alias::new("authorization_run_reads"))
		.and_where(Expr::col("resource_kind").eq("artifact"))
		.to_string(PostgresQueryBuilder);
	let index = Query::delete()
		.from_table(Alias::new("semantic_entries"))
		.and_where(Expr::cust("source->>'kind' = 'artifact'"))
		.to_string(PostgresQueryBuilder);
	let events = Query::delete()
		.from_table(Alias::new("events"))
		.and_where(Expr::col("kind").eq("artifact.published"))
		.to_string(PostgresQueryBuilder);
	format!(
		"{SCOPE}ALTER TABLE public.registry DISABLE TRIGGER marketplace_registry_fence;\n\
		 {registry};\n\
		 ALTER TABLE public.registry ENABLE TRIGGER marketplace_registry_fence;\n\
		 {context};\n{summary};\n{reads};\n{index};\n{events};\n"
	)
}

pub(crate) fn federated_reads() -> String {
	let reads = Query::delete()
		.from_table(Alias::new("authorization_remote_grant_reads"))
		.and_where(Expr::col("resource_kind").eq("artifact"))
		.to_string(PostgresQueryBuilder);
	format!("{SCOPE}{reads};\n")
}

pub(crate) fn federation_settlement() -> String {
	let receipts = Query::delete()
		.from_table(Alias::new("authorization_remote_commands"))
		.and_where(Expr::col("request_key").like("run_message_complete:%"))
		.to_string(PostgresQueryBuilder);
	format!("{SCOPE}{receipts};\n")
}
