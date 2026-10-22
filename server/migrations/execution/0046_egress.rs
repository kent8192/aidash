// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0046_egress", "execution")
		.add_dependency("execution", "0045_decision_routes_state")
		.add_dependency("identity", "0038_tenant_secrets_state")
		.atomic(true)
		.database_only(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_operations".into(),
			constraint: Constraint::Unique {
				name: "core_operations_tenant_id_key".into(),
				columns: vec!["tenant".into(), "id".into()],
			},
		})
		.add_operation(Operation::CreateTable {
			name: "egress_budgets".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("operation_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("service", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("deadline_at", FieldType::TimestampTz)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("sent_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("received_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("requests", FieldType::Integer)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("max_sent_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("max_received_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("max_requests", FieldType::Integer)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![
				Constraint::Check {
					name: "egress_counters".into(),
					expression: "sent_bytes >= 0 AND received_bytes >= 0".into(),
				},
				Constraint::ForeignKey {
					name: "egress_budgets_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "egress_budgets_operation_fkey".into(),
					columns: vec!["tenant".into(), "operation_id".into()],
					referenced_table: "core_operations".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "egress_requests".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("run_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("operation_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("service", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("method", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("path", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("query_digest", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("status", FieldType::Integer)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("sent_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("received_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("duration_ms", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("error", FieldType::Text)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![
				Constraint::Check {
					name: "egress_counters".into(),
					expression: "sent_bytes >= 0 AND received_bytes >= 0".into(),
				},
				Constraint::ForeignKey {
					name: "egress_requests_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "egress_requests_operation_fkey".into(),
					columns: vec!["tenant".into(), "operation_id".into()],
					referenced_table: "core_operations".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "egress_budgets".into(),
			name: "egress_budget_operation_service".into(),
			columns: vec!["tenant".into(), "operation_id".into(), "service".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "egress_requests".into(),
			name: "egress_requests_run".into(),
			columns: vec!["tenant".into(), "run_id".into(), "id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
