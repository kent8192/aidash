// reinhardt-migration-source: 1
// PostgreSQL function bodies, NOT VALID JSON checks and the catalog-driven
// installation guard allowlist rewrite require DDL SQL assets. The Agent
// contract keeps the 0017 Projection Version key.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0018_deferred_exposure", "registry")
		.add_dependency("registry", "0017_projection_versions")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0018_deferred_exposure.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0018_deferred_exposure.sql").into()),
		})
}
