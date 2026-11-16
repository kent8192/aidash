// reinhardt-migration-source: 1
// External services are reached only through the Execution Pod proxy (#738).
// The per-operation budgets, the Live preview session budgets and the session
// reference of an Egress request go. The row-level-security policies have no
// typed operation, so they are a SQL asset.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0134_remove_legacy_egress_tokens", "execution")
		.add_dependency("execution", "0133_agent_plugin_data_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0134_remove_legacy_egress_tokens.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0134_remove_legacy_egress_tokens.sql").into(),
			),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "egress_requests".into(),
			constraint: Constraint::ForeignKey {
				name: "egress_requests_session_fkey".into(),
				columns: vec!["tenant".into(), "session_id".into()],
				referenced_table: "egress_sessions".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropColumn {
			table: "egress_requests".into(),
			column: "session_id".into(),
			old_definition: Some(ColumnDefinition::new("session_id", FieldType::Uuid)),
		})
		// Only the removed operation route wrote an operation; a Scratch Space
		// request names its Run alone, so Node purge follows the Run.
		.add_operation(Operation::DropConstraintDefinition {
			table: "egress_requests".into(),
			constraint: Constraint::ForeignKey {
				name: "egress_requests_operation_fkey".into(),
				columns: vec!["tenant".into(), "operation_id".into()],
				referenced_table: "core_operations".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropColumn {
			table: "egress_requests".into(),
			column: "operation_id".into(),
			old_definition: Some(ColumnDefinition::new("operation_id", FieldType::Uuid)),
		})
		.add_operation(Operation::DropColumn {
			table: "core_live_previews".into(),
			column: "egress_expires_at".into(),
			old_definition: Some(ColumnDefinition::new(
				"egress_expires_at",
				FieldType::TimestampTz,
			)),
		})
		// The historical model state omits this database default; remove it
		// explicitly so reversing the drop restores the original table schema.
		.add_operation(Operation::AlterColumn {
			table: "egress_sessions".into(),
			column: "created_at".into(),
			old_definition: Some(
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".into())),
			),
			new_definition: ColumnDefinition::new("created_at", FieldType::TimestampTz)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "egress_sessions".into(),
			constraint: Constraint::ForeignKey {
				name: "egress_sessions_preview_fkey".into(),
				columns: vec!["tenant".into(), "live_preview_id".into()],
				referenced_table: "core_live_previews".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "egress_sessions".into(),
			constraint: Constraint::Unique {
				name: "egress_sessions_tenant_id_key".into(),
				columns: vec!["tenant".into(), "id".into()],
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "egress_sessions".into(),
			constraint: Constraint::ForeignKey {
				name: "egress_sessions_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropTable {
			name: "egress_sessions".into(),
		})
		.add_operation(Operation::DropNamedIndex {
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
		.add_operation(Operation::DropConstraintDefinition {
			table: "egress_budgets".into(),
			constraint: Constraint::Check {
				name: "egress_counters".into(),
				expression: "sent_bytes >= 0 AND received_bytes >= 0".into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "egress_budgets".into(),
			constraint: Constraint::ForeignKey {
				name: "egress_budgets_tenant_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "egress_budgets".into(),
			constraint: Constraint::ForeignKey {
				name: "egress_budgets_operation_fkey".into(),
				columns: vec!["tenant".into(), "operation_id".into()],
				referenced_table: "core_operations".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropTable {
			name: "egress_budgets".into(),
		})
}
