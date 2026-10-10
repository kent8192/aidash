// reinhardt-migration-source: 1
// Preserve applied definitions, including the Provider Credential, Projection
// Version and cache_mode allowlist keys of 0016-0018; widen only the model config
// allowlist with the optional `streaming` and `stream_stall_timeout_secs`
// transport settings, validated by the separate registry_model_streaming check.
// PostgreSQL JSONB CHECK expressions and function bodies live in SQL assets.
// The filesystem migration loader supports external assets through RunSQL only.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0019_model_streaming_config", "registry")
		.database_only(true)
		.add_dependency("registry", "0018_prompt_cache")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0019_model_streaming_config.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0019_model_streaming_config.sql").into()),
		})
		.atomic(true)
}
