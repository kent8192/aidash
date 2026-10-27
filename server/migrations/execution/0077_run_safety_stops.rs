// reinhardt-migration-source: 1
// The audit record and replay receipt of a Safety Stop (#485, ADR 0101). One
// row per accepted or no-op command. `run_id` deliberately has no foreign key:
// the record is audit history that outlives the Run and its Workspace, and is
// erased only by `audit_retention_days` after the Tenant is purged. The Tenant
// root reference follows every other Tenant-owned table. The logical model
// state is kept by 0079.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0077_run_safety_stops", "execution")
		.add_dependency("execution", "0076_run_control_trigger")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "run_safety_stops".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("action", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("expected_control_revision", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("actor_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("identity_id", FieldType::Uuid),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("operation_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("caller_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("fingerprint", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "run_safety_stops_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Unique {
					name: "run_safety_stops_operation".into(),
					columns: vec!["tenant".into(), "caller_key".into(), "operation_id".into()],
				},
				Constraint::Check {
					name: "run_safety_stops_record".into(),
					expression: "action IN ('pause', 'cancel') AND outcome IN ('accepted', 'no_op') AND actor_kind IN ('shared_bearer', 'browser') AND (actor_kind = 'browser') = (identity_id IS NOT NULL) AND char_length(reason) BETWEEN 1 AND 1000 AND expected_control_revision >= 0".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "run_safety_stops".into(),
			columns: vec!["tenant".into(), "created_at".into(), "id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "run_safety_stops".into(),
			columns: vec!["tenant".into(), "run_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
