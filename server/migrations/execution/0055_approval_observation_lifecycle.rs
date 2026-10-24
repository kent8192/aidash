// reinhardt-migration-source: 1
// Observer evidence follows the owning Run's retention and Tenant lifecycle.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0055_approval_observation_lifecycle", "execution")
		.add_dependency("execution", "0054_approval_observation_row_level_security")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "approval_observations".into(),
			constraint: Constraint::Unique {
				name: "approval_observation_tenant_id".into(),
				columns: vec!["tenant".into(), "id".into()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "approval_observations".into(),
			constraint: Constraint::ForeignKey {
				name: "approval_observations_run".into(),
				columns: vec!["tenant".into(), "run_id".into()],
				referenced_table: "runs".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "approval_observation_population".into(),
			constraint: Constraint::ForeignKey {
				name: "approval_observation_population_run".into(),
				columns: vec!["tenant".into(), "run_id".into()],
				referenced_table: "runs".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "approval_observation_attempts".into(),
			constraint: Constraint::ForeignKey {
				name: "approval_observation_attempt_owner".into(),
				columns: vec!["tenant".into(), "observation".into()],
				referenced_table: "approval_observations".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
}
