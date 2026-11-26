// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

pub(super) fn migration() -> Migration {
	let registry = Query::update()
		.table(Alias::new("registry"))
		.value_expr(
			Alias::new("metadata"),
			Expr::cust("jsonb_set(metadata, '{config}', (metadata->'config') - 'completion')"),
		)
		.and_where(Expr::col("kind").eq("agent"))
		.and_where(Expr::cust("metadata->'config' ? 'completion'"))
		.to_string(PostgresQueryBuilder);
	let execution = Query::update()
		.table(Alias::new("task_executions"))
		.value_expr(Alias::new("context"), Expr::cust("context - 'completion'"))
		.and_where(Expr::cust("context ? 'completion'"))
		.to_string(PostgresQueryBuilder);
	let summary = Query::update()
		.table(Alias::new("task_executions"))
		.value_expr(Alias::new("context"), Expr::cust(
			"jsonb_set(context, '{execution_summary,content}', 			 (context #> '{execution_summary,content}') - 'goal' || 			 jsonb_build_object('intent', context #> '{execution_summary,content,goal}'))",
		))
		.and_where(Expr::cust("context #> '{execution_summary,content,goal}' IS NOT NULL"))
		.to_string(PostgresQueryBuilder);
	let artifact_reads = Query::delete()
		.from_table(Alias::new("authorization_run_reads"))
		.and_where(Expr::col("resource_kind").eq("artifact"))
		.to_string(PostgresQueryBuilder);
	let artifact_index = Query::delete()
		.from_table(Alias::new("semantic_entries"))
		.and_where(Expr::cust("source->>'kind' = 'artifact'"))
		.to_string(PostgresQueryBuilder);
	let events = Query::delete()
		.from_table(Alias::new("events"))
		.and_where(Expr::col("kind").eq("artifact.published"))
		.to_string(PostgresQueryBuilder);

	Migration::new("0172_retire_completion_gate", "execution")
		.add_dependency("execution", "0171_outside_versions")
		.add_dependency("registry", "0082_thread_source_scope")
		.add_dependency("workspaces", "0071_completion_messages")
		.add_dependency("identity", "0123_goal_judge_model")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::RunSQL {
			// Reinhardt Query produces the data updates. PostgreSQL migration scope,
			// trigger lifecycle and validation function replacements require SQL.
			sql: format!(
				"SET LOCAL aidash.scope = 'deployment';\n\
				 ALTER TABLE public.registry DISABLE TRIGGER marketplace_registry_fence;\n\
				 {registry};\n\
				 ALTER TABLE public.registry ENABLE TRIGGER marketplace_registry_fence;\n\
				 {execution};\n{summary};\n{artifact_reads};\n{artifact_index};\n{events};\n{}",
				include_str!("sql/forward/0172_goal_registry_contract.sql"),
			),
			reverse_sql: Some(include_str!("sql/backward/0172_goal_registry_contract.sql").into()),
		})
}
