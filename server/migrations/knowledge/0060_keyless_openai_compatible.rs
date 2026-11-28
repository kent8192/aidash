// reinhardt-migration-source: 1
// A semantic index's Keyless embedding names `openai_compatible` (#1169), as
// registry/0083 does for embedding definitions. The first operation extends
// the embedding provider allowlist of semantic_indexes_revision: a
// catalog-driven JSONB CHECK edit has no typed Reinhardt operation, so that
// DDL is SQL. The second rewrites index embeddings without a Provider
// Credential that said `openrouter` or `openai`; it is Reinhardt Query output
// from `knowledge::services::keyless_embedding_backfill`, checked by
// `migrationseeds`. The reverse puts them on `openai`, which every earlier
// version accepted.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0060_keyless_openai_compatible", "knowledge")
		.add_dependency("knowledge", "0059_threads_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0060_keyless_provider_checks.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0060_keyless_provider_checks.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0060_keyless_openai_compatible.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0060_keyless_openai_compatible.sql").into(),
			),
		})
}
