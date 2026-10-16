// reinhardt-migration-source: 1
// PostgreSQL function bodies have no typed migration operation, so the Agent
// contract replacement lives in SQL assets. The body keeps the 0017 Projection
// Version, 0018 prompt_cache, 0019 deferred exposure, 0021 tool_parallelism
// and 0022 Context Policy keys and adds the #112 Completion Policy. It follows
// the #243 removal of packages and installations (0026/0027), which itself
// follows 0025_compactor_provider_keys, and 0028_tool_transport_secrets, so its
// reversal reads Agent definitions only.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0029_completion_policy", "registry")
		.add_dependency("registry", "0028_tool_transport_secrets")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0029_completion_policy.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0029_completion_policy.sql").into()),
		})
}
