// reinhardt-migration-source: 1
// Record the declared ModelEnum change; database checks are applied in 0008.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0009_provider_model_state", "registry")
		.state_only(true)
		.add_dependency("registry", "0008_provider_descriptors")
		.add_operation(Operation::DropConstraintDefinition {
			table: "registry".into(),
			constraint: Constraint::EnumDomain {
				name: "registry_kind_model_enum_check".into(),
				column: "kind".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("agent".into()),
						ModelEnumValue::String("cluster".into()),
						ModelEnumValue::String("compactor".into()),
						ModelEnumValue::String("embedding".into()),
						ModelEnumValue::String("model".into()),
						ModelEnumValue::String("node".into()),
						ModelEnumValue::String("skill".into()),
						ModelEnumValue::String("tool".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry".into(),
			constraint: Constraint::EnumDomain {
				name: "registry_kind_model_enum_check".into(),
				column: "kind".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("agent".into()),
						ModelEnumValue::String("bundle".into()),
						ModelEnumValue::String("cluster".into()),
						ModelEnumValue::String("compactor".into()),
						ModelEnumValue::String("embedding".into()),
						ModelEnumValue::String("memory".into()),
						ModelEnumValue::String("model".into()),
						ModelEnumValue::String("node".into()),
						ModelEnumValue::String("skill".into()),
						ModelEnumValue::String("source".into()),
						ModelEnumValue::String("tool".into()),
					],
				},
			},
		})
}
