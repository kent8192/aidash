// reinhardt-migration-source: 1
// Upload reservations outlive a publishing transaction so interrupted writes
// remain visible to recovery. PostgreSQL row policies require raw DDL.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0098_blob_uploads", "execution")
		.add_dependency("execution", "0097_blob_receipt_references")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "core_blob_uploads".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text),
			],
			constraints: vec![
				Constraint::Check {
					name: "core_blob_uploads_size_check".into(),
					expression: "size >= 0".into(),
				},
				Constraint::ForeignKey {
					name: "core_blob_uploads_tenant_fkey".into(),
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
			table: "core_blob_uploads".into(),
			name: "core_blob_uploads_tenant_digest".into(),
			columns: vec!["tenant".into(), "digest".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0098_blob_uploads.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0098_blob_uploads.sql").into()),
		})
}
