// reinhardt-migration-source: 1
// ORM state for the approval carry-over columns in 0073.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0074_approval_carry_over_state", "identity")
		.add_dependency("identity", "0073_approval_carry_over")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog".into(),
			column: ColumnDefinition::new("carry_over", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog".into(),
			column: ColumnDefinition::new("scope", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog".into(),
			column: ColumnDefinition::new("carried_from_version", FieldType::Text)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog".into(),
			column: ColumnDefinition::new("carried_from_approver", FieldType::Text)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog_history".into(),
			column: ColumnDefinition::new("carry_over", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog_history".into(),
			column: ColumnDefinition::new("scope", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog_history".into(),
			column: ColumnDefinition::new("carried_from_version", FieldType::Text)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog_history".into(),
			column: ColumnDefinition::new("carried_from_approver", FieldType::Text)
				.with_not_null(false),
			mysql_options: None,
		})
}
