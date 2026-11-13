// reinhardt-migration-source: 1
// The ORM state of the table 0124 creates.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0125_scope_refusals_state", "execution")
		.add_dependency("execution", "0124_scope_refusals")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "egress_scope_refusals".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("minute", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("host", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("count", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
