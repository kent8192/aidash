// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation, so the Agent
// contract replacement lives in SQL assets. The body keeps the 0017 Projection
// Version, 0018 prompt_cache, 0019 deferred exposure, 0021 tool_parallelism
// and 0022 Context Policy keys and adds the #112 Completion Policy. It follows
// both registry leaves, 0025_compactor_provider_keys and the #243 removal of
// packages and installations, so its reversal reads Agent definitions only.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0027_completion_policy", "registry")
		.add_dependency("registry", "0025_compactor_provider_keys")
		.add_dependency("registry", "0026_remove_legacy_installations_state")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0027_completion_policy.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0027_completion_policy.sql").into()),
		})
}
