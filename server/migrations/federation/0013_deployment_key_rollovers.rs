// reinhardt-migration-source: 1
//! Deployment-wide durable sender schedule. No Tenant ownership or RLS policy.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0013_deployment_key_rollovers", "federation")
		.add_dependency("federation", "0012_foreign_key_relations_state")
		.database_only(true)
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
