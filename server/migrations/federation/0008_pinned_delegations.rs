// reinhardt-migration-source: 1
//! Retain an Agent Tool's exact receiver closure across delivery retries.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0008_pinned_delegations", "federation")
		.database_only(true)
		.add_dependency("identity", "0011_home_human_requests_state")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").into()),
		})
		.add_operation(Operation::AddColumn {
			table: "delegations".into(),
			column: ColumnDefinition::new("binding_snapshot", FieldType::Jsonb),
			mysql_options: None,
		})
}
