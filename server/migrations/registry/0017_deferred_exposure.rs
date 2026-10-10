// reinhardt-migration-source: 1
// PostgreSQL function bodies and NOT VALID JSON checks require DDL SQL assets.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0017_deferred_exposure", "registry")
		.add_dependency("registry", "0016_provider_credentials")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0017_deferred_exposure.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0017_deferred_exposure.sql").into()),
		})
}
