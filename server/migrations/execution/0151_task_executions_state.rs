// reinhardt-migration-source: 1
// ORM state of the renamed table's enum-domain checks; the database has them
// as `task_executions_phase_check` and `task_executions_control_check` (0150).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0151_task_executions_state", "execution")
		.add_dependency("execution", "0150_task_executions")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "task_executions".into(),
			constraint: Constraint::EnumDomain {
				name: "runs_phase_model_enum_check".into(),
				column: "phase".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("CANCELLED".into()),
						ModelEnumValue::String("COMPLETED".into()),
						ModelEnumValue::String("FAILED".into()),
						ModelEnumValue::String("READY".into()),
						ModelEnumValue::String("THINKING".into()),
						ModelEnumValue::String("TOOL_CALL".into()),
						ModelEnumValue::String("WAITING".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "task_executions".into(),
			constraint: Constraint::EnumDomain {
				name: "task_executions_phase_model_enum_check".into(),
				column: "phase".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("CANCELLED".into()),
						ModelEnumValue::String("COMPLETED".into()),
						ModelEnumValue::String("FAILED".into()),
						ModelEnumValue::String("READY".into()),
						ModelEnumValue::String("THINKING".into()),
						ModelEnumValue::String("TOOL_CALL".into()),
						ModelEnumValue::String("WAITING".into()),
					],
				},
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "task_executions".into(),
			constraint: Constraint::EnumDomain {
				name: "runs_control_model_enum_check".into(),
				column: "control".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("ACTIVE".into()),
						ModelEnumValue::String("CANCELLED".into()),
						ModelEnumValue::String("PAUSED".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "task_executions".into(),
			constraint: Constraint::EnumDomain {
				name: "task_executions_control_model_enum_check".into(),
				column: "control".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("ACTIVE".into()),
						ModelEnumValue::String("CANCELLED".into()),
						ModelEnumValue::String("PAUSED".into()),
					],
				},
			},
		})
}
