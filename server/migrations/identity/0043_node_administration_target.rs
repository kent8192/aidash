// reinhardt-migration-source: 1
// Node management audit targets the permanent name claim, never the purged row.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0043_node_administration_target", "identity")
		.add_dependency("identity", "0042_node_deletions_row_level_security")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "dashboard_administration_history".into(),
			column: ColumnDefinition::new("node", FieldType::Text).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_administration_history".into(),
			constraint: Constraint::ForeignKey {
				name: "dashboard_administration_history_node_owner_fkey".into(),
				columns: vec!["node".into(), "tenant".into()],
				referenced_table: "node_names".into(),
				referenced_columns: vec!["name".into(), "tenant".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "dashboard_administration_history".into(),
			constraint: Constraint::Check {
				name: "dashboard_administration_history_node_tenant".into(),
				expression: "node IS NULL OR tenant IS NOT NULL".into(),
			},
		})
}
