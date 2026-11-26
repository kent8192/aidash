// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0175_node_drives", "execution")
		.atomic(true)
		.database_only(true)
		.add_dependency("execution", "0163_scope_host_debit_state")
		.add_dependency("identity", "0115_suspension_resume_refusal")
		.add_operation(Operation::CreateTable {
			name: "drives".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("closed", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("head", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![
				Constraint::Unique { name: "drives_node_key".into(), columns: vec!["tenant".into(), "node".into()] },
				Constraint::Unique { name: "drives_tenant_id_key".into(), columns: vec!["tenant".into(), "id".into()] },
				Constraint::ForeignKey { name: "drives_nodes_fkey".into(), columns: vec!["tenant".into(), "node".into()], referenced_table: "nodes".into(), referenced_columns: vec!["tenant".into(), "name".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_versions".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("version", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("author", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("change_count", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("operation_id", FieldType::Uuid).with_not_null(false),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "drive_versions_drives_fkey".into(), columns: vec!["tenant".into(), "drive_id".into()], referenced_table: "drives".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Unique { name: "drive_versions_version_key".into(), columns: vec!["drive_id".into(), "version".into()] },
				Constraint::Unique { name: "drive_versions_operation_key".into(), columns: vec!["drive_id".into(), "operation_id".into()] },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_entries".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("path", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("media_type", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("added_in_version", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("removed_in_version", FieldType::BigInteger).with_not_null(false),
				ColumnDefinition::new("indexed", FieldType::Boolean).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "drive_entries_drives_fkey".into(), columns: vec!["tenant".into(), "drive_id".into()], referenced_table: "drives".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::ForeignKey { name: "drive_entries_core_blobs_fkey".into(), columns: vec!["tenant".into(), "digest".into()], referenced_table: "core_blobs".into(), referenced_columns: vec!["tenant".into(), "digest".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Unique { name: "drive_entries_version_path_key".into(), columns: vec!["drive_id".into(), "added_in_version".into(), "path".into()] },
				Constraint::Check { name: "drive_entries_lifetime".into(), expression: "size >= 0 AND added_in_version > 0 AND (removed_in_version IS NULL OR removed_in_version > added_in_version)".into() },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_trash".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("path", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("removed_in_version", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "drive_trash_drives_fkey".into(), columns: vec!["tenant".into(), "drive_id".into()], referenced_table: "drives".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::ForeignKey { name: "drive_trash_drive_entries_fkey".into(), columns: vec!["entry_id".into()], referenced_table: "drive_entries".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_uploads".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("author", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("spec", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("published_version", FieldType::BigInteger).with_not_null(false),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "drive_uploads_drives_fkey".into(), columns: vec!["tenant".into(), "drive_id".into()], referenced_table: "drives".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Unique { name: "drive_uploads_tenant_id_key".into(), columns: vec!["tenant".into(), "id".into()] },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_upload_parts".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("upload_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("number", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "drive_upload_parts_drive_uploads_fkey".into(), columns: vec!["tenant".into(), "upload_id".into()], referenced_table: "drive_uploads".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::ForeignKey { name: "drive_upload_parts_core_blobs_fkey".into(), columns: vec!["tenant".into(), "digest".into()], referenced_table: "core_blobs".into(), referenced_columns: vec!["tenant".into(), "digest".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Unique { name: "drive_upload_parts_number_key".into(), columns: vec!["upload_id".into(), "number".into()] },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_jobs".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("author", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("plan", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "drive_jobs_drives_fkey".into(), columns: vec!["tenant".into(), "drive_id".into()], referenced_table: "drives".into(), referenced_columns: vec!["tenant".into(), "id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_job_blobs".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("job_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey { name: "drive_job_blobs_drive_jobs_fkey".into(), columns: vec!["job_id".into()], referenced_table: "drive_jobs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::ForeignKey { name: "drive_job_blobs_core_blobs_fkey".into(), columns: vec!["tenant".into(), "digest".into()], referenced_table: "core_blobs".into(), referenced_columns: vec!["tenant".into(), "digest".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
				Constraint::Unique { name: "drive_job_blobs_digest_key".into(), columns: vec!["job_id".into(), "digest".into()] },
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "drive_entries".into(), name: "drive_entries_drive_path".into(),
			columns: vec!["drive_id".into(), "path".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None,
			concurrently: false, expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "drive_entries".into(), name: "drive_entries_current_path".into(),
			columns: vec!["drive_id".into(), "path".into()], unique: true,
			index_type: Some(IndexType::BTree), where_clause: Some("removed_in_version IS NULL".into()),
			concurrently: false, expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "drive_entries".into(), name: "drive_entries_tenant_digest".into(),
			columns: vec!["tenant".into(), "digest".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None,
			concurrently: false, expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "drive_upload_parts".into(), name: "drive_upload_parts_tenant_digest".into(),
			columns: vec!["tenant".into(), "digest".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None,
			concurrently: false, expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "drive_job_blobs".into(), name: "drive_job_blobs_tenant_digest".into(),
			columns: vec!["tenant".into(), "digest".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None,
			concurrently: false, expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "drive_versions".into(), name: "drive_versions_task".into(),
			columns: vec!["tenant".into(), "task_id".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None,
			concurrently: false, expressions: None, mysql_options: None, operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "drive_trash".into(), name: "drive_trash_expires".into(),
			columns: vec!["expires_at".into()], unique: false,
			index_type: Some(IndexType::BTree), where_clause: None,
			concurrently: false, expressions: None, mysql_options: None, operator_class: None,
		})
		// Reinhardt has no operations for forced PostgreSQL row-level security.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0175_node_drives.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0175_node_drives.sql").into()),
		})
}
