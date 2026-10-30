// reinhardt-migration-source: 1
// ORM Binary metadata for the physical PostgreSQL bytea columns.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0054_reference_image_byte_state", "registry")
		.add_dependency("registry", "0053_reference_images")
		.add_operation(Operation::AlterColumn {
			table: "agent_knowledge".into(),
			column: "image_0".into(),
			old_definition: Some(
				ColumnDefinition::new("image_0", FieldType::Bytea).with_not_null(false),
			),
			new_definition: ColumnDefinition::new("image_0", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "agent_knowledge".into(),
			column: "image_1".into(),
			old_definition: Some(
				ColumnDefinition::new("image_1", FieldType::Bytea).with_not_null(false),
			),
			new_definition: ColumnDefinition::new("image_1", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "agent_knowledge".into(),
			column: "image_2".into(),
			old_definition: Some(
				ColumnDefinition::new("image_2", FieldType::Bytea).with_not_null(false),
			),
			new_definition: ColumnDefinition::new("image_2", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "agent_knowledge".into(),
			column: "image_3".into(),
			old_definition: Some(
				ColumnDefinition::new("image_3", FieldType::Bytea).with_not_null(false),
			),
			new_definition: ColumnDefinition::new("image_3", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "agent_knowledge".into(),
			column: "image_4".into(),
			old_definition: Some(
				ColumnDefinition::new("image_4", FieldType::Bytea).with_not_null(false),
			),
			new_definition: ColumnDefinition::new("image_4", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "agent_knowledge".into(),
			column: "image_5".into(),
			old_definition: Some(
				ColumnDefinition::new("image_5", FieldType::Bytea).with_not_null(false),
			),
			new_definition: ColumnDefinition::new("image_5", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "agent_knowledge".into(),
			column: "image_6".into(),
			old_definition: Some(
				ColumnDefinition::new("image_6", FieldType::Bytea).with_not_null(false),
			),
			new_definition: ColumnDefinition::new("image_6", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "agent_knowledge".into(),
			column: "image_7".into(),
			old_definition: Some(
				ColumnDefinition::new("image_7", FieldType::Bytea).with_not_null(false),
			),
			new_definition: ColumnDefinition::new("image_7", FieldType::Binary)
				.with_not_null(false),
			mysql_options: None,
		})
		.state_only(true)
		.atomic(true)
}
