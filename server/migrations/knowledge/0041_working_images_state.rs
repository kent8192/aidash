// reinhardt-migration-source: 1
// Keep Reinhardt model state aligned with the image indexing state.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0041_working_images_state", "knowledge")
		.add_dependency("knowledge", "0040_working_images")
		.add_operation(Operation::DropConstraintDefinition {
			table: "semantic_entries".into(),
			constraint: Constraint::EnumDomain {
				name: "semantic_entries_state_model_enum_check".into(),
				column: "state".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("DELETED".into()),
						ModelEnumValue::String("ERROR".into()),
						ModelEnumValue::String("PENDING".into()),
						ModelEnumValue::String("READY".into()),
						ModelEnumValue::String("REVOKED".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_entries".into(),
			constraint: Constraint::EnumDomain {
				name: "semantic_entries_state_model_enum_check".into(),
				column: "state".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("DELETED".into()),
						ModelEnumValue::String("ERROR".into()),
						ModelEnumValue::String("PENDING".into()),
						ModelEnumValue::String("READY".into()),
						ModelEnumValue::String("REVOKED".into()),
						ModelEnumValue::String("UNSUPPORTED".into()),
					],
				},
			},
		})
		.state_only(true)
		.atomic(true)
}
