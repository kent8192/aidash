// reinhardt-migration-source: 1
//! The ORM schema. Drive query indexes, Tenant fences and foreign keys are
//! installed by the database-only migrations.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0177_drive_model_state", "execution")
		.add_dependency("execution", "0176_drive_reads")
		.state_only(true)
		.add_operation(Operation::CreateTable {
			name: "drives".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("closed", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("head", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_versions".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
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
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_entries".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("path", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("media_type", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("added_in_version", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("removed_in_version", FieldType::BigInteger)
					.with_not_null(false),
				ColumnDefinition::new("indexed", FieldType::Boolean).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_trash".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("path", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("removed_in_version", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_uploads".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("drive_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("author", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("spec", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("published_version", FieldType::BigInteger)
					.with_not_null(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_upload_parts".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("upload_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("number", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_jobs".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
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
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_job_blobs".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("job_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "drive_run_reads".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("thread_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Uuid).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
