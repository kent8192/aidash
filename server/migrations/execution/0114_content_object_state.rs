// reinhardt-migration-source: 1
// Revision manifests are no longer columns on the logical Area model.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0114_content_object_state", "execution")
		.add_dependency("execution", "0113_content_object_references")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropColumn {
			table: "core_areas".into(),
			column: "manifest".into(),
			old_definition: Some(
				ColumnDefinition::new("manifest", FieldType::Jsonb).with_not_null(true),
			),
		})
}
