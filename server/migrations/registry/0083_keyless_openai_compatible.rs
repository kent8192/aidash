// reinhardt-migration-source: 1
// Keyless model and embedding definitions name `openai_compatible` (#1169), so
// Aidash sends a Keyless endpoint nothing that only OpenRouter understands.
// The first operation extends the provider allowlists of
// registry_model_config and registry_embedding_config: catalog-driven JSONB
// CHECK edits have no typed Reinhardt operation, so that DDL is SQL. The
// second rewrites stored definitions without a grant (models on `openrouter`,
// embeddings on `openrouter` or `openai`) in `registry` and
// `registry_requests`; it is Reinhardt Query output from
// `registry::services::keyless_provider_backfill`, checked by
// `migrationseeds`. A Keyless model also loses its `provider_route` and an
// explicit `cache_mode`, which only an OpenRouter route can use. SET LOCAL
// opens the Deployment Scope, and the Marketplace fence is disabled only
// around the `registry` rewrite, as in registry/0054. Published Marketplace
// versions and revisions are immutable: their content and digests no longer
// match a rewritten Registry copy, so affected versions need republication.
// The reverse restores `openrouter` on models and `openai` on embeddings; it
// cannot restore the removed keys or which embeddings said `openrouter`.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0083_keyless_openai_compatible", "registry")
		.add_dependency("registry", "0082_thread_source_scope")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0083_keyless_provider_checks.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0083_keyless_provider_checks.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0083_keyless_openai_compatible.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0083_keyless_openai_compatible.sql").into(),
			),
		})
}
