// reinhardt-migration-source: 1
// Preserve applied definitions; widen only the embedding provider allowlist.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0024_openrouter_embeddings", "knowledge")
        .add_dependency("knowledge", "0023_native_memory_model_state")
        // PostgreSQL JSONB CHECK expressions live in SQL assets. The filesystem
        // migration loader supports external assets through RunSQL only.
        .add_operation(Operation::RunSQL {
            sql: include_str!("sql/forward/0024_openrouter_embeddings.sql").into(),
            reverse_sql: Some(include_str!("sql/backward/0024_openrouter_embeddings.sql").into()),
        })
        .atomic(true)
        .database_only(true)
}
