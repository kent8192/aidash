// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0173_authorization_read_model_state", "execution")
		.add_dependency("execution", "0172_retire_completion_gate")
		.state_only(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "authorization_run_reads".into(),
			constraint: Constraint::EnumDomain {
				name: "authorization_run_reads_resource_kind_model_enum_check".into(),
				column: "resource_kind".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("artifact".into()),
						ModelEnumValue::String("conversation".into()),
						ModelEnumValue::String("generation".into()),
						ModelEnumValue::String("message".into()),
						ModelEnumValue::String("run".into()),
						ModelEnumValue::String("task".into()),
						ModelEnumValue::String("workspace_events".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_run_reads".into(),
			constraint: Constraint::EnumDomain {
				name: "authorization_run_reads_resource_kind_model_enum_check".into(),
				column: "resource_kind".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("conversation".into()),
						ModelEnumValue::String("generation".into()),
						ModelEnumValue::String("message".into()),
						ModelEnumValue::String("run".into()),
						ModelEnumValue::String("task".into()),
						ModelEnumValue::String("workspace_events".into()),
					],
				},
			},
		})
}
