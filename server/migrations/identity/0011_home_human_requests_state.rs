// reinhardt-migration-source: 1
//! Model state for the Home continuation journal; physical rows default to [].
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0011_home_human_requests_state", "identity")
		.add_dependency("identity", "0010_home_human_requests")
		.add_operation(Operation::AddColumn {
			table: "authorization_remote_execution".into(),
			column: ColumnDefinition::new("human_requests", FieldType::Jsonb).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "delegations".into(),
			column: ColumnDefinition::new("human_requests", FieldType::Jsonb).with_not_null(true),
			mysql_options: None,
		})
		.state_only(true)
}
