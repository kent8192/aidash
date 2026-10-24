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
				columns: vec!["tenant".into(), "bank_id".into()],
				referenced_table: "memory_banks".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Restrict,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_unit_retention".into(),
			constraint: Constraint::ForeignKey {
				name: "retention_unit".into(),
				columns: vec!["tenant".into(), "unit_id".into()],
				referenced_table: "memory_units".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_unit_retention".into(),
			constraint: Constraint::ForeignKey {
				name: "retention_bank".into(),
				columns: vec!["tenant".into(), "bank_id".into()],
				referenced_table: "memory_banks".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
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
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_bank_decay".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "memory_bank_decay_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "memory_unit_retention".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "memory_unit_retention_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.atomic(true)
		.state_only(true)
}
