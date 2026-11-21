// reinhardt-migration-source: 1
//! Subnodes are no longer capped per Run or in members (#1012, ADR 0143):
//! only the receiving Tenant's consent to Subnodes that remote work creates
//! remains in `subnode_limits`. Aidash is undeployed, so the reverse restores
//! the columns with their former defaults and range check.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0143_subnode_caps_removed", "execution")
		.add_dependency("execution", "0142_remove_research_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "subnode_limits".into(),
			constraint: Constraint::Check {
				name: "subnode_limits_range".into(),
				expression:
					"members_per_subnode BETWEEN 1 AND 20 AND subnodes_per_run BETWEEN 1 AND 10"
						.into(),
			},
		})
		.add_operation(Operation::DropColumn {
			table: "subnode_limits".into(),
			column: "members_per_subnode".into(),
			old_definition: Some(
				ColumnDefinition::new("members_per_subnode", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("5".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "subnode_limits".into(),
			column: "subnodes_per_run".into(),
			old_definition: Some(
				ColumnDefinition::new("subnodes_per_run", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("3".into())),
			),
		})
}
