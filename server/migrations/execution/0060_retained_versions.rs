// reinhardt-migration-source: 1
// Retained versions (ADR 0095): a reference keeps a File version by hold. The
// message trigger is DDL that typed operations cannot express and is the only
// cascade-release for a holder whose row the workspaces app owns.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0060_retained_versions", "execution")
		.add_dependency("execution", "0059_research_history_merge")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::CreateTable {
			name: "core_retention_holds".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("area_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("holder_kind", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("holder_id", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("peer", FieldType::Text)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![
				Constraint::Check {
					name: "retention_holder_kind".into(),
					expression:
						"holder_kind IN ('message', 'live_preview', 'federation')".into(),
				},
				Constraint::Check {
					name: "retention_federation_holder".into(),
					expression:
						"(holder_kind = 'federation') = (expires_at IS NOT NULL) AND (holder_kind = 'federation') = (peer IS NOT NULL)"
							.into(),
				},
				// One hold per holder per Working Area revision: a holder may cite
				// File versions of several Areas or revisions, while every File
				// version of one revision belongs to the same hold. Files are
				// unique per hold below, so concurrent identical retains converge.
				Constraint::Unique {
					name: "core_retention_holds_holder_revision_key".into(),
					columns: vec![
						"tenant".into(),
						"area_id".into(),
						"holder_kind".into(),
						"holder_id".into(),
						"revision".into(),
					],
				},
				Constraint::ForeignKey {
					name: "core_retention_holds_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "core_retention_holds_area_fkey".into(),
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
					columns: vec!["hold_id".into()],
					referenced_table: "core_retention_holds".into(),
					referenced_columns: vec!["id".into()],
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
					columns: vec!["file_id".into()],
					referenced_table: "core_objects".into(),
					referenced_columns: vec!["id".into()],
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
			table: "core_retention_holds".into(),
			name: "core_retention_holds_area".into(),
			columns: vec!["tenant".into(), "area_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "core_retention_holds".into(),
			name: "core_retention_holds_holder".into(),
			columns: vec!["tenant".into(), "holder_kind".into(), "holder_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "core_retention_holds".into(),
			name: "core_retention_holds_expiry".into(),
			columns: vec!["expires_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
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
			sql: include_str!("sql/forward/0060_retained_versions.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0060_retained_versions.sql").to_owned(),
			),
		})
}
