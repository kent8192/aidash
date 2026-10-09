// reinhardt-migration-source: 1
// Match scalar ORM metadata without removing the physical FK and Usage checks.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0026_memory_decay_model_state", "knowledge")
		.add_dependency("knowledge", "0025_memory_decay")
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_bank_decay".into(),
			constraint: Constraint::ForeignKey {
				name: "decay_bank".into(),
				columns: vec!["bank_id".into()],
				referenced_table: "memory_banks".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_unit_retention".into(),
			constraint: Constraint::ForeignKey {
				name: "retention_unit".into(),
				columns: vec!["unit_id".into()],
				referenced_table: "memory_units".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_unit_retention".into(),
			constraint: Constraint::ForeignKey {
				name: "retention_bank".into(),
				columns: vec!["bank_id".into()],
				referenced_table: "memory_banks".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_unit_retention".into(),
			constraint: Constraint::Check {
				name: "retention_deliveries".into(),
				expression: "deliveries >= 0 AND deliveries < 9223372036854775807".into(),
			},
		})
		.atomic(true)
		.state_only(true)
}
