// reinhardt-migration-source: 1
// Undeployed UUID object bytes are discarded. References and quota rows are
// recreated because they refer to those objects. PostgreSQL policies and the
// atomic-write statement triggers require DDL unavailable in Reinhardt.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0091_content_object_references", "execution")
		.add_dependency("execution", "0090_content_blobs")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_blobs".into(),
			constraint: Constraint::Unique {
				name: "core_blobs_tenant_digest_size_key".into(),
				columns: vec!["tenant".into(), "digest".into(), "size".into()],
			},
		})
		.add_operation(Operation::DropColumn {
			table: "core_areas".into(),
			column: "manifest".into(),
			old_definition: Some(
				ColumnDefinition::new("manifest", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some("'[]'::jsonb".into())),
			),
		})
		// Reversing DropTable restores columns, constraints and indexes from
		// the model state, not the triggers and policies 0006, 0036 and 0062
		// added with RunSQL. Their backward assets drop them, so restore them.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0091_content_object_references_legacy.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0091_content_object_references_legacy.sql").into(),
			),
		})
		.add_operation(Operation::DropTable {
			name: "core_retention_files".into(),
		})
		.add_operation(Operation::DropTable {
			name: "core_objects".into(),
		})
		.add_operation(Operation::DropTable {
			name: "core_quotas".into(),
		})
		.add_operation(Operation::CreateTable {
			name: "core_objects".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".into())),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "core_objects_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Unique {
					name: "core_objects_tenant_id_key".into(),
					columns: vec!["tenant".into(), "id".into()],
				},
				Constraint::Unique {
					name: "core_objects_tenant_id_digest_key".into(),
					columns: vec!["tenant".into(), "id".into(), "digest".into()],
				},
				Constraint::ForeignKey {
					name: "core_objects_blob_fkey".into(),
					columns: vec!["tenant".into(), "digest".into(), "size".into()],
					referenced_table: "core_blobs".into(),
					referenced_columns: vec!["tenant".into(), "digest".into(), "size".into()],
					on_delete: ForeignKeyAction::Restrict,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddColumn {
			table: "core_blob_references".into(),
			column: ColumnDefinition::new("object_id", FieldType::Uuid),
            mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_blob_references".into(),
			constraint: Constraint::ForeignKey {
				name: "core_blob_references_object_fkey".into(),
				columns: vec!["tenant".into(), "object_id".into(), "digest".into()],
				referenced_table: "core_objects".into(),
				referenced_columns: vec!["tenant".into(), "id".into(), "digest".into()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
        .add_operation(Operation::AddColumn { table: "core_blob_references".into(), column: ColumnDefinition::new("area_id", FieldType::Uuid), mysql_options: None })
        .add_operation(Operation::AddColumn { table: "core_blob_references".into(), column: ColumnDefinition::new("revision", FieldType::BigInteger), mysql_options: None })
        .add_operation(Operation::AddConstraintDefinition {
            table: "core_blob_references".into(),
            constraint: Constraint::ForeignKey { name: "core_blob_references_revision_fkey".into(), columns: vec!["tenant".into(), "area_id".into(), "revision".into()], referenced_table: "core_manifest_revisions".into(), referenced_columns: vec!["tenant".into(), "area_id".into(), "revision".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
        })
        .add_operation(Operation::AddConstraintDefinition {
            table: "core_blob_references".into(),
            constraint: Constraint::Check { name: "core_blob_references_revision_check".into(), expression: "holder_kind <> 'revision' OR (area_id IS NOT NULL AND revision IS NOT NULL AND object_id IS NOT NULL)".into() },
        })
		.add_operation(Operation::CreateTable {
			name: "core_quotas".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("used_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
			],
			constraints: vec![Constraint::ForeignKey {
				name: "core_quotas_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_retention_files".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("hold_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("file_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("path", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("size", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("media_type", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![
				Constraint::Check {
					name: "retention_file_size".into(),
					expression: "size >= 0".into(),
				},
				Constraint::Unique {
					name: "core_retention_files_hold_file_key".into(),
					columns: vec!["hold_id".into(), "file_id".into()],
				},
				Constraint::ForeignKey {
					name: "core_retention_files_hold_fkey".into(),
					columns: vec!["tenant".into(), "hold_id".into()],
					referenced_table: "core_retention_holds".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "core_retention_files_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "core_retention_files_object_fkey".into(),
					columns: vec!["tenant".into(), "file_id".into()],
					referenced_table: "core_objects".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
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
			table: "core_retention_files".into(),
			name: "core_retention_files_object".into(),
			columns: vec!["tenant".into(), "file_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0091_content_object_references_policy.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0091_content_object_references_policy.sql").into(),
			),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0091_content_object_references_triggers.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0091_content_object_references_triggers.sql").into(),
			),
		})
}
