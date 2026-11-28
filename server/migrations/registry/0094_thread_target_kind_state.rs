// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0094_thread_target_kind_state", "registry")
		.add_dependency("registry", "0093_registry_kind_state")
		.add_dependency("workspaces", "0067_task_duty_start")
		.atomic(true)
		.state_only(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "conversations".into(),
			constraint: Constraint::EnumDomain {
				name: "conversations_target_kind_model_enum_check".into(),
				column: "target_kind".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("agent".into()),
						ModelEnumValue::String("cluster".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "conversations".into(),
			constraint: Constraint::EnumDomain {
				name: "conversations_target_kind_model_enum_check".into(),
				column: "target_kind".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("agent".into()),
						ModelEnumValue::String("node".into()),
					],
				},
			},
		})
}
