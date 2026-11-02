// reinhardt-migration-source: 1
// Receipt references disappear with their owning Tenant record, never earlier.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0097_blob_receipt_references", "execution")
		.add_dependency("execution", "0096_blob_reservations_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_records".into(),
			constraint: Constraint::Unique {
				name: "core_records_tenant_id_key".into(),
				columns: vec!["tenant".into(), "id".into()],
			},
		})
		.add_operation(Operation::AddColumn {
			table: "core_blob_references".into(),
			column: ColumnDefinition::new("record_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_blob_references".into(),
			constraint: Constraint::ForeignKey {
				name: "core_blob_references_record_fkey".into(),
				columns: vec!["tenant".into(), "record_id".into()],
				referenced_table: "core_records".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "core_blob_references".into(),
			name: "core_blob_references_object".into(),
			columns: vec!["tenant".into(), "object_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "core_blob_references".into(),
			name: "core_blob_references_record".into(),
			columns: vec!["tenant".into(), "record_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "core_blob_references".into(),
			name: "core_blob_references_revision".into(),
			columns: vec!["tenant".into(), "area_id".into(), "revision".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "core_objects".into(),
			name: "core_objects_tenant_digest".into(),
			columns: vec!["tenant".into(), "digest".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_blob_references".into(),
			constraint: Constraint::Check {
				name: "core_blob_references_receipt_check".into(),
				expression:
					"holder_kind <> 'receipt' OR (record_id IS NOT NULL AND object_id IS NOT NULL)"
						.into(),
			},
		})
}
