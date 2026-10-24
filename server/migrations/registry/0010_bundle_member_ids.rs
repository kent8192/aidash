// reinhardt-migration-source: 1
// PostgreSQL function bodies are unsupported by typed migration operations.
// Replace validation DDL only; historical package and definition bytes stay intact.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0010_bundle_member_ids", "registry")
		.database_only(true)
		.add_dependency("registry", "0009_provider_model_state")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0010_bundle_member_ids.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0010_bundle_member_ids.sql").to_owned()),
		})
}
