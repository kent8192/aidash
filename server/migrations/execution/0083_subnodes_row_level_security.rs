// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0083_subnodes_row_level_security", "execution")
		.add_dependency("execution", "0082_subnodes")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0083_subnodes_row_level_security.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0083_subnodes_row_level_security.sql").to_owned(),
			),
		})
}
