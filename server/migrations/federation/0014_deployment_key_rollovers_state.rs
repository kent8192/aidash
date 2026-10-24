// reinhardt-migration-source: 1
//! ORM model state for the Deployment-wide sender schedule.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0014_deployment_key_rollovers_state", "federation")
		.add_dependency("federation", "0013_deployment_key_rollovers")
		.state_only(true)
		.add_operation(Operation::CreateTable {
			name: "deployment_key_rollovers".into(),
			columns: vec![
				ColumnDefinition::new("deployment", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
