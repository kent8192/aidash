// reinhardt-migration-source: 1
//! Model state for `0133_subnode_caps_removed` (#1012).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0134_subnode_caps_removed_state", "execution")
		.add_dependency("execution", "0133_subnode_caps_removed")
		.state_only(true)
		.add_operation(Operation::DropColumn {
			table: "subnode_limits".into(),
			column: "members_per_subnode".into(),
			old_definition: Some(
				ColumnDefinition::new("members_per_subnode", FieldType::Integer)
					.with_not_null(true),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "subnode_limits".into(),
			column: "subnodes_per_run".into(),
			old_definition: Some(
				ColumnDefinition::new("subnodes_per_run", FieldType::Integer).with_not_null(true),
			),
		})
}
