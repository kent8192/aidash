// reinhardt-migration-source: 1
// #108 Part B: the `compactor` kind is removed and compaction runs through a
// `decider` definition (ADR 0042). A Decider's configuration is validated by
// the application, like other provider definitions, so only the kind list
// changes here, in both kind checks, and the compactor configuration check is
// dropped. Aidash is not deployed, so there is no data rewrite: adding the new
// checks fails while an old compactor row exists, and such entries must be
// registered again as Deciders (or the development database reset). Typed
// constraint operations suffice; the dropped checks carry their definitions
// so the migration reverses.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0031_decider_kind", "registry")
		.database_only(true)
		.add_dependency("registry", "0030_plan_connection_models")
		.add_operation(Operation::DropConstraintDefinition {
			table: "registry".into(),
			constraint: Constraint::Check {
				name: "registry_compactor_config".into(),
				expression: r#"COALESCE(((kind <> 'compactor'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider_credential'::text, 'model'::text, 'max_request_bytes'::text, 'max_questions'::text, 'max_response_bytes'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof(((metadata -> 'config'::text) -> 'provider_credential'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'provider_credential'::text) = 'openrouter'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'model'::text) ~ '^typesafe/jev-(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)$'::text) AND (octet_length(((metadata -> 'config'::text) ->> 'model'::text)) <= 256) AND
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_request_bytes'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'max_request_bytes'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'max_request_bytes'::text))::text)::numeric >= (1024)::numeric) AND (((((metadata -> 'config'::text) -> 'max_request_bytes'::text))::text)::numeric <= (1048576)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_questions'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'max_questions'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'max_questions'::text))::text)::numeric >= (1)::numeric) AND (((((metadata -> 'config'::text) -> 'max_questions'::text))::text)::numeric <= (1024)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_response_bytes'::text)) = 'number'::text) AND ((((metadata -> 'config'::text) -> 'max_response_bytes'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN ((((((metadata -> 'config'::text) -> 'max_response_bytes'::text))::text)::numeric >= (128)::numeric) AND (((((metadata -> 'config'::text) -> 'max_response_bytes'::text))::text)::numeric <= (1048576)::numeric))
						ELSE false
						END)), false)"#.into(),
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "registry".into(),
			constraint: Constraint::Check {
				name: "registry_kind_check".into(),
				expression: "kind IN ('agent','model','tool','skill','cluster','node','compactor','embedding','bundle','memory','source','reranker','tokenizer')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "registry".into(),
			constraint: Constraint::Check {
				name: "registry_kind_check".into(),
				expression: "kind IN ('agent','model','tool','skill','cluster','node','decider','embedding','bundle','memory','source','reranker','tokenizer')".into(),
			},
		})
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
						ModelEnumValue::String("compactor".into()),
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
