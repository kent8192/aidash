// reinhardt-migration-source: 1
// Tenant blob ownership and immutable Area revisions. Reinhardt has no
// operation for PostgreSQL row-level policies, so those use documented DDL.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0090_content_blobs", "execution")
		.add_dependency("execution", "0089_remove_agent_cluster")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::CreateTable {
			name: "core_blobs".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![
				Constraint::PrimaryKey {
					name: "core_blobs_pkey".into(),
					columns: vec!["tenant".into(), "digest".into()],
				},
				Constraint::Check {
					name: "core_blobs_size_check".into(),
					expression: "size >= 0".into(),
				},
				Constraint::Check {
					name: "core_blobs_digest_check".into(),
					expression: "digest ~ '^[0-9a-f]{64}$'".into(),
				},
				Constraint::ForeignKey {
					name: "core_blobs_tenant_fkey".into(),
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
		.add_operation(Operation::CreateTable {
			name: "core_manifest_revisions".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("manifest", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![
				Constraint::PrimaryKey {
					name: "core_manifest_revisions_pkey".into(),
					columns: vec!["tenant".into(), "area_id".into(), "revision".into()],
				},
				Constraint::ForeignKey {
					name: "core_manifest_revisions_area_fkey".into(),
					columns: vec!["tenant".into(), "area_id".into()],
					referenced_table: "core_areas".into(),
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
		.add_operation(Operation::CreateTable {
			name: "core_blob_references".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("holder_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("holder_id", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::Unique {
					name: "core_blob_references_holder_key".into(),
					columns: vec![
						"tenant".into(),
						"digest".into(),
						"holder_kind".into(),
						"holder_id".into(),
					],
				},
				Constraint::ForeignKey {
					name: "core_blob_references_blob_fkey".into(),
					columns: vec!["tenant".into(), "digest".into()],
					referenced_table: "core_blobs".into(),
					referenced_columns: vec!["tenant".into(), "digest".into()],
					on_delete: ForeignKeyAction::Restrict,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0090_content_blobs.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0090_content_blobs.sql").into()),
		})
}
