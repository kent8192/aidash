// reinhardt-migration-source: 1
// PostgreSQL functions and NOT VALID constraints lack typed migration operations.
// DDL only; obsolete historical rows remain intact and confer no new authority.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0011_binding_contract", "registry")
		.database_only(true)
		.add_dependency("registry", "0010_bundle_member_ids")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0011_binding_contract.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0011_binding_contract.sql").into()),
		})
}
