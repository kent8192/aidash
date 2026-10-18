// reinhardt-migration-source: 1
// Reinhardt makemigrations model state for the `decider` definition kind.
// The physical kind check is applied by 0033_decider_kind.

use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0034_decider_kind_state", "registry")
		.add_operation(Operation::DropConstraintDefinition {
			table: "registry".to_string(),
			constraint: Constraint::EnumDomain {
				name: "registry_kind_model_enum_check".to_string(),
				column: "kind".to_string(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("agent".to_string()),
						ModelEnumValue::String("bundle".to_string()),
						ModelEnumValue::String("cluster".to_string()),
						ModelEnumValue::String("compactor".to_string()),
						ModelEnumValue::String("embedding".to_string()),
						ModelEnumValue::String("memory".to_string()),
						ModelEnumValue::String("model".to_string()),
						ModelEnumValue::String("node".to_string()),
						ModelEnumValue::String("reranker".to_string()),
						ModelEnumValue::String("skill".to_string()),
						ModelEnumValue::String("source".to_string()),
						ModelEnumValue::String("tokenizer".to_string()),
						ModelEnumValue::String("tool".to_string()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry".to_string(),
			constraint: Constraint::EnumDomain {
				name: "registry_kind_model_enum_check".to_string(),
				column: "kind".to_string(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("agent".to_string()),
						ModelEnumValue::String("bundle".to_string()),
						ModelEnumValue::String("cluster".to_string()),
						ModelEnumValue::String("decider".to_string()),
						ModelEnumValue::String("embedding".to_string()),
						ModelEnumValue::String("memory".to_string()),
						ModelEnumValue::String("model".to_string()),
						ModelEnumValue::String("node".to_string()),
						ModelEnumValue::String("reranker".to_string()),
						ModelEnumValue::String("skill".to_string()),
						ModelEnumValue::String("source".to_string()),
						ModelEnumValue::String("tokenizer".to_string()),
						ModelEnumValue::String("tool".to_string()),
					],
				},
			},
		})
		.add_dependency("registry", "0033_decider_kind")
		.atomic(true)
		.with_initial(None)
		.state_only(true)
		.database_only(false)
}
