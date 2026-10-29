// reinhardt-migration-source: 1
// A receiving Node pins the sending Deployment's dashboard origin before it
// allows that dashboard to frame one of its File or Live previews.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0013_registered_preview_origin", "federation")
		.add_dependency("federation", "0012_foreign_key_relations_state")
		.add_operation(Operation::AddColumn {
			table: "node_registrations".into(),
			column: ColumnDefinition::new("source_dashboard_origin", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "node_registration_history".into(),
			column: ColumnDefinition::new("source_dashboard_origin", FieldType::Text),
			mysql_options: None,
		})
		.atomic(true)
}
