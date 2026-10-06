// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0009_memory_role_references", "registry")
		.add_dependency("registry", "0008_memory_roles")
		.add_operation(Operation::CreateTable {
			name: "registry_memory_role_refs".into(),
			columns: vec![
				ColumnDefinition::new("source_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("role", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("ordinal", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("required_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reference_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reference_version", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::PrimaryKey { name: "registry_memory_role_refs_pkey".into(), columns: vec!["source_id".into(), "source_version".into(), "role".into(), "ordinal".into()] },
				Constraint::ForeignKey { name: "registry_memory_role_source".into(), columns: vec!["source_id".into(), "source_version".into()], referenced_table: "registry".into(), referenced_columns: vec!["id".into(), "version".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
				Constraint::ForeignKey { name: "registry_memory_role_target".into(), columns: vec!["reference_id".into(), "reference_version".into(), "required_kind".into()], referenced_table: "registry".into(), referenced_columns: vec!["id".into(), "version".into(), "kind".into()], on_delete: ForeignKeyAction::Restrict, on_update: ForeignKeyAction::Restrict, deferrable: None },
			],
			without_rowid: None, interleave_in_parent: None, partition: None,
		})
		// Trigger functions cannot be represented by the typed migration API.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0009_memory_role_references.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0009_memory_role_references.sql").into()),
		})
		.atomic(true)
}
