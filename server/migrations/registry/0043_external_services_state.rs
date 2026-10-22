// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0043_external_services_state", "registry")
		.add_dependency("registry", "0042_external_services")
		.atomic(true)
		.state_only(true)
		.add_operation(Operation::DropConstraintDefinition {
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
						ModelEnumValue::String("decider".into()),
						ModelEnumValue::String("embedding".into()),
						ModelEnumValue::String("memory".into()),
						ModelEnumValue::String("model".into()),
						ModelEnumValue::String("node".into()),
						ModelEnumValue::String("reranker".into()),
						ModelEnumValue::String("skill".into()),
						ModelEnumValue::String("source".into()),
						ModelEnumValue::String("tokenizer".into()),
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
						ModelEnumValue::String("decider".into()),
						ModelEnumValue::String("embedding".into()),
						ModelEnumValue::String("external_service".into()),
						ModelEnumValue::String("memory".into()),
						ModelEnumValue::String("model".into()),
						ModelEnumValue::String("node".into()),
						ModelEnumValue::String("reranker".into()),
						ModelEnumValue::String("skill".into()),
						ModelEnumValue::String("source".into()),
						ModelEnumValue::String("tokenizer".into()),
						ModelEnumValue::String("tool".into()),
					],
				},
			},
		})
}
