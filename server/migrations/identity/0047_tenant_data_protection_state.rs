// reinhardt-migration-source: 1
// ORM state of the Tenant policy and history; physical checks and RLS are
// database-only in 0041 and 0042.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0047_tenant_data_protection_state", "identity")
		.add_dependency("identity", "0046_data_protection_row_level_security")
		.state_only(true)
		.add_operation(Operation::CreateTable {
			name: "tenant_data_protection".into(),
			columns: vec![
				ColumnDefinition::new("level", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("model_call_zdr", FieldType::Boolean)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("plan_connection_exempt", FieldType::Boolean)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
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
			name: "tenant_data_protection_history".into(),
			columns: vec![
				ColumnDefinition::new("changed_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("changed_by", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("level", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("model_call_zdr", FieldType::Boolean)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("plan_connection_exempt", FieldType::Boolean)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
