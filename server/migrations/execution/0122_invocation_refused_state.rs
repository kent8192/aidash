// reinhardt-migration-source: 1
// Keep the ORM domain in sync with the additional invocation status.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0122_invocation_refused_state", "execution")
		.add_dependency("execution", "0121_invocation_refused")
		.atomic(true)
		.state_only(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "invocations".into(),
			constraint: Constraint::EnumDomain {
				name: "invocations_status_model_enum_check".into(),
				column: "status".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("COMPLETED".into()),
						ModelEnumValue::String("STARTED".into()),
						ModelEnumValue::String("UNCERTAIN".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "invocations".into(),
			constraint: Constraint::EnumDomain {
				name: "invocations_status_model_enum_check".into(),
				column: "status".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("COMPLETED".into()),
						ModelEnumValue::String("REFUSED".into()),
						ModelEnumValue::String("STARTED".into()),
						ModelEnumValue::String("UNCERTAIN".into()),
					],
				},
			},
		})
}
