// reinhardt-migration-source: 1
// Preserve physical defaults and the check while matching ORM model state.
use reinhardt::db::migrations::{FieldType, prelude::*};

pub(super) fn migration() -> Migration {
	Migration::new("0047_image_publications_state", "knowledge")
		.add_dependency("knowledge", "0046_image_publication_lookup")
		.add_operation(Operation::AlterColumn {
			table: "semantic_image_ledgers".into(),
			column: "next_authority_check".into(),
			old_definition: None,
			new_definition: ColumnDefinition::new("next_authority_check", FieldType::TimestampTz).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "semantic_image_publications".into(),
			column: "created_at".into(),
			old_definition: None,
			new_definition: ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "semantic_image_publications".into(),
			constraint: Constraint::Check {
				name: "semantic_image_publications_shape".into(),
				expression: "generation > 0 AND CASE WHEN jsonb_typeof(changes) = 'array' THEN jsonb_array_length(changes) > 0 ELSE false END AND jsonb_typeof(authority) = 'object'".into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "semantic_image_ledgers".into(),
			constraint: Constraint::ForeignKey {
				name: "semantic_image_ledgers_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "semantic_image_publications".into(),
			constraint: Constraint::ForeignKey {
				name: "semantic_image_publications_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.state_only(true)
		.atomic(true)
}
