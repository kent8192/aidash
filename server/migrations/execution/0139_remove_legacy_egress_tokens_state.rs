// reinhardt-migration-source: 1
// Logical model state; 0138 owns the physical change.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0139_remove_legacy_egress_tokens_state", "execution")
		.add_dependency("execution", "0138_remove_legacy_egress_tokens")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "egress_sessions".into(),
		})
		.add_operation(Operation::DropTable {
			name: "egress_budgets".into(),
		})
		.add_operation(Operation::DropColumn {
			table: "egress_requests".into(),
			column: "session_id".into(),
			old_definition: Some(ColumnDefinition::new("session_id", FieldType::Uuid)),
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
}
