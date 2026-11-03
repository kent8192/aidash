// reinhardt-migration-source: 1
// ORM state for the optional Node audit target. Physical references stay in 0043.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0059_node_administration_target_state", "identity")
		.add_dependency("identity", "0058_node_administration_target")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "dashboard_administration_history".into(),
			column: ColumnDefinition::new("node", FieldType::Text).with_not_null(false),
			mysql_options: None,
		})
}
