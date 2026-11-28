// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0120_tenant_embedding_settings", "identity")
		.add_dependency("identity", "0115_suspension_resume_refusal")
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "tenant_embedding_settings".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("active", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("pending", FieldType::Jsonb),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("next_attempt", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
