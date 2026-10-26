// reinhardt-migration-source: 1
// A thread message cites exact File versions (ADR 0095). The citation records
// the path, digest, size, media type and Working Area revision seen when it was
// written, so a later publication can never change what the message refers to.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0011_message_file_versions", "workspaces")
		.add_dependency("workspaces", "0010_foreign_key_relations_state")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::CreateTable {
			name: "message_file_versions".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("message_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("file_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("area_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("path", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("digest", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("size", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("media_type", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![
				Constraint::Unique {
					name: "message_file_versions_message_file_key".into(),
					columns: vec!["message_id".into(), "file_id".into()],
				},
				Constraint::Check {
					name: "message_file_version_size".into(),
					expression: "size >= 0".into(),
				},
				Constraint::ForeignKey {
					name: "message_file_versions_message_fkey".into(),
					columns: vec!["message_id".into()],
					referenced_table: "messages".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "message_file_versions_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "message_file_versions".into(),
			name: "message_file_versions_message".into(),
			columns: vec!["tenant".into(), "message_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
