//! Reinhardt Query output for the Goal and Completion Message cutover.
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

const SCOPE: &str = "SET LOCAL aidash.scope = 'deployment';\n";

pub(crate) fn execution() -> String {
	let config = "(metadata->'config') - 'completion'";
	let remove_default = format!(
		"CASE WHEN jsonb_typeof(metadata #> '{{config,remove_default}}') = 'array' THEN \
         jsonb_set({config}, '{{remove_default}}', (metadata #> '{{config,remove_default}}') - 'artifact_publish') \
         ELSE {config} END"
	);
	let bindings = format!(
		"CASE WHEN jsonb_typeof(metadata #> '{{config,bindings}}') = 'array' THEN \
         jsonb_set({remove_default}, '{{bindings}}', COALESCE((SELECT jsonb_agg(binding ORDER BY position) \
         FROM jsonb_array_elements(metadata #> '{{config,bindings}}') WITH ORDINALITY AS b(binding, position) \
         WHERE binding #>> '{{target,id}}' IS DISTINCT FROM 'aidash.artifact_publish'), '[]'::jsonb)) \
         ELSE {remove_default} END"
	);
	let registry = Query::update()
		.table(Alias::new("registry"))
		.value_expr(
			Alias::new("metadata"),
			Expr::cust(format!(
				"jsonb_set(metadata - 'binding_normalization', '{{config}}', {bindings})"
			)),
		)
		.and_where(Expr::col("kind").eq("agent"))
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
	let checks = Query::update()
		.table(Alias::new("core_operations"))
		.value_expr(Alias::new("state"), Expr::value("cancelled"))
		.and_where(Expr::col("kind").eq("verification"))
		.and_where(Expr::col("state").is_in(["prepared", "submitted", "running", "cancelling"]))
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
		 {context};\n{summary};\n{checks};\n{reads};\n{index};\n{events};\n"
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
