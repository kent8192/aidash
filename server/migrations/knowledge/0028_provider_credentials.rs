// reinhardt-migration-source: 1
// PostgreSQL JSONB predicates and catalog-driven CHECK DDL are not expressible
// in Reinhardt SchemaExpr. Preserve the current index contract in SQL assets.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0028_provider_credentials", "knowledge")
		.add_dependency("knowledge", "0027_memory_retention_lookup")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0028_provider_credentials.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0028_provider_credentials.sql").into()),
		})
		.atomic(true)
		.database_only(true)
}
