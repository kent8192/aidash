// reinhardt-migration-source: 1
// The physical purpose constraint changes in 0080; keep the ORM enum state aligned.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0086_reference_embedding_usage_state", "execution")
		.add_dependency("execution", "0085_reference_embedding_usage")
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_embedding_usage".into(),
			constraint: Constraint::EnumDomain {
				name: "generation_embedding_usage_purpose_model_enum_check".into(),
				column: "purpose".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("index".into()),
						ModelEnumValue::String("query".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".into(),
			constraint: Constraint::EnumDomain {
				name: "generation_embedding_usage_purpose_model_enum_check".into(),
				column: "purpose".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("index".into()),
						ModelEnumValue::String("query".into()),
						ModelEnumValue::String("reference".into()),
					],
				},
			},
		})
		.state_only(true)
		.atomic(true)
}
