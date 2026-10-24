// reinhardt-migration-source: 1
//! Context source declarations (#110, ADR 0086). PostgreSQL validation function
//! bodies have no typed schema operation; this DDL changes no application data.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0045_context_sources", "registry")
		.database_only(true)
		.add_dependency("registry", "0044_foreign_key_relations_state")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0045_context_sources.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0045_context_sources.sql").into()),
		})
		.atomic(true)
}
