// reinhardt-migration-source: 1
// ORM state of Tenant and Node Instructions (#1010); physical keys, checks and
// RLS are database-only in 0082 and 0083.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0093_standing_instructions_state", "identity")
		.add_dependency("identity", "0092_standing_instructions_row_level_security")
		.state_only(true)
		.add_operation(Operation::CreateTable {
			name: "node_instructions".into(),
			columns: vec![
				ColumnDefinition::new("node", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("text", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("updated_by", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "tenant_instructions".into(),
			columns: vec![
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("text", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("updated_by", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
