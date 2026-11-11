// reinhardt-migration-source: 1
// Model state for Host Access reservations; physical checks stay database-only.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0075_bootstrap_reservations_state", "identity")
		.add_dependency("identity", "0074_bootstrap_reservations")
		.state_only(true)
		.add_operation(Operation::AddColumn {
			table: "demo_reservations".into(),
			column: ColumnDefinition::new("source", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::CreateTable {
			name: "bootstrap_deployment_administrator_reservations".into(),
			columns: vec![
				ColumnDefinition::new("email", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("issuer", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.atomic(true)
}
