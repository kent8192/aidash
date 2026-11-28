// reinhardt-migration-source: 1
// Discard obsolete Working Area manifests, file retention and the queue cursor.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0184_retire_area_holds", "execution")
		.add_dependency("execution", "0183_detach_run_area_bindings")
		.atomic(true)
		.add_operation(Operation::RunSQL {
			// Reinhardt has no operation for trigger functions or RLS policies.
			sql: include_str!("sql/backward/0061_retained_versions.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0184_restore_area_holds.sql").into()),
		})
		.add_operation(Operation::DropNamedIndex {
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
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_files".into(),
			constraint: Constraint::ForeignKey {
					name: "core_retention_files_object_fkey".into(),
					columns: vec!["tenant".into(), "file_id".into()],
					referenced_table: "core_objects".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_files".into(),
			constraint: Constraint::ForeignKey {
					name: "core_retention_files_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_files".into(),
			constraint: Constraint::ForeignKey {
					name: "core_retention_files_hold_fkey".into(),
					columns: vec!["tenant".into(), "hold_id".into()],
					referenced_table: "core_retention_holds".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_files".into(),
			constraint: Constraint::Unique {
					name: "core_retention_files_hold_file_key".into(),
					columns: vec!["hold_id".into(), "file_id".into()],
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_files".into(),
			constraint: Constraint::Check {
					name: "retention_file_size".into(),
					expression: "size >= 0".into(),
				},
		})
		.add_operation(Operation::DropTable {
			name: "core_retention_files".into(),
		})
		.add_operation(Operation::DropNamedIndex {
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
		.add_operation(Operation::DropNamedIndex {
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
		.add_operation(Operation::DropNamedIndex {
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
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_holds".into(),
			constraint: Constraint::ForeignKey {
					name: "core_retention_holds_area_fkey".into(),
					columns: vec!["tenant".into(), "area_id".into()],
					referenced_table: "core_areas".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_holds".into(),
			constraint: Constraint::ForeignKey {
					name: "core_retention_holds_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_holds".into(),
			constraint: Constraint::Unique {
					name: "core_retention_holds_tenant_id_key".into(),
					columns: vec!["tenant".into(), "id".into()],
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_holds".into(),
			constraint: Constraint::Unique {
					name: "core_retention_holds_holder_revision_key".into(),
					columns: vec![
						"tenant".into(),
						"area_id".into(),
						"holder_kind".into(),
						"holder_id".into(),
						"revision".into(),
					],
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_holds".into(),
			constraint: Constraint::Check {
					name: "retention_federation_holder".into(),
					expression:
						"(holder_kind = 'federation') = (expires_at IS NOT NULL) AND (holder_kind = 'federation') = (peer IS NOT NULL)"
							.into(),
				},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_holds".into(),
			constraint: Constraint::Check {
					name: "retention_holder_kind".into(),
					expression:
						"holder_kind IN ('message', 'live_preview', 'federation')".into(),
				},
		})
		.add_operation(Operation::DropTable {
			name: "core_retention_holds".into(),
		})
		.add_operation(Operation::RunSQL {
			// Generated from the Reinhardt Query seed. The old contents are disposable.
			sql: include_str!("sql/forward/0184_retire_area_manifests.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0184_retire_area_manifests.sql").into(),
			),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_blob_references".into(),
			constraint: Constraint::ForeignKey {
				name: "core_blob_references_revision_fkey".into(),
				columns: vec!["tenant".into(), "area_id".into(), "revision".into()],
				referenced_table: "core_manifest_revisions".into(),
				referenced_columns: vec!["tenant".into(), "area_id".into(), "revision".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropTable {
			name: "core_manifest_revisions".into(),
		})
		.add_operation(Operation::DropColumn {
			table: "core_areas".into(),
			column: "next_sequence".into(),
			old_definition: Some(
				ColumnDefinition::new("next_sequence", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".into())),
			),
		})
		.add_operation(Operation::RenameTable {
			old_name: "core_areas".into(),
			new_name: "core_execution_scopes".into(),
		})
		.add_operation(Operation::RunSQL {
			// Reinhardt has no operation for renaming a view.
			sql: "ALTER VIEW public.core_run_areas RENAME TO core_run_scopes;".into(),
			reverse_sql: Some(
				"ALTER VIEW public.core_run_scopes RENAME TO core_run_areas;".into(),
			),
		})
}
