// reinhardt-migration-source: 1
// Keep the ORM domain in sync with the additional human-request kind.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0084_mcp_connection_requests_state", "execution")
		.add_dependency("execution", "0083_mcp_connection_requests")
		.atomic(true)
		.state_only(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "human_requests".into(),
			constraint: Constraint::EnumDomain {
				name: "human_requests_kind_model_enum_check".into(),
				column: "kind".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("APPROVAL_REQUIRED".into()),
						ModelEnumValue::String("CONFIRMATION".into()),
						ModelEnumValue::String("INFORMATION_REQUEST".into()),
						ModelEnumValue::String("MCP_ELICITATION".into()),
						ModelEnumValue::String("QUESTION".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "human_requests".into(),
			constraint: Constraint::EnumDomain {
				name: "human_requests_kind_model_enum_check".into(),
				column: "kind".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("APPROVAL_REQUIRED".into()),
						ModelEnumValue::String("CONFIRMATION".into()),
						ModelEnumValue::String("INFORMATION_REQUEST".into()),
						ModelEnumValue::String("MCP_CONNECTION".into()),
						ModelEnumValue::String("MCP_ELICITATION".into()),
						ModelEnumValue::String("QUESTION".into()),
					],
				},
			},
		})
}
