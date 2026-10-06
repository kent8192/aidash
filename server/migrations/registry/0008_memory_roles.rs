// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0008_memory_roles", "registry")
		.add_dependency("registry", "0007_model_state")
		.add_operation(Operation::DropConstraint { table: "registry".into(), constraint_name: "registry_kind_check".into() })
		.add_operation(Operation::AddConstraintDefinition { table: "registry".into(), constraint: Constraint::Check {
			name: "registry_kind_check".into(),
			expression: "kind IN ('agent','model','tool','skill','cluster','node','compactor','embedding','memory','source','reranker','tokenizer')".into(),
		} })
		.add_operation(Operation::DropConstraint { table: "registry".into(), constraint_name: "registry_agent_config".into() })
		.add_operation(Operation::AddConstraintDefinition { table: "registry".into(), constraint: Constraint::Check { name: "registry_agent_config".into(), expression: r#"COALESCE((((kind <> 'agent'::text) OR ((((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((length(btrim(COALESCE(((metadata -> 'config'::text) ->> 'instructions'::text), ''::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) OR
						CASE
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skills'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skills'::text)) > 0)
						ELSE false
						END) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text]))) OR ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'instructions'::text), '""'::jsonb)) = 'string'::text) AND ((COALESCE(((metadata -> 'config'::text) -> 'skill_attachments'::text), '[]'::jsonb) <> '[]'::jsonb) OR (COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb) <> '[]'::jsonb)))) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'knowledge_digest'::text), 'null'::jsonb)) = ANY (ARRAY['string'::text, 'null'::text])) AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'max_steps'::text)) THEN true
						WHEN ((jsonb_typeof(((metadata -> 'config'::text) -> 'max_steps'::text)) = 'number'::text) AND (((metadata -> 'config'::text) ->> 'max_steps'::text) ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((metadata -> 'config'::text) ->> 'max_steps'::text))::numeric >= (1)::numeric) AND ((((metadata -> 'config'::text) ->> 'max_steps'::text))::numeric <= (1000)::numeric))
						ELSE false
						END)) AND ((kind <> 'agent'::text) OR (((NOT ((metadata -> 'config'::text) ? 'core_capabilities'::text)) OR (
						CASE
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'core_capabilities'::text)) = 'object'::text) THEN ((((metadata -> 'config'::text) -> 'core_capabilities'::text) - ARRAY['files'::text, 'shell'::text, 'python'::text, 'patch'::text, 'skills'::text, 'sharing'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'files'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'files'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'shell'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'shell'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'python'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'python'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'patch'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'patch'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'skills'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'skills'::text)) = 'boolean'::text)) AND ((NOT (((metadata -> 'config'::text) -> 'core_capabilities'::text) ? 'sharing'::text)) OR (jsonb_typeof((((metadata -> 'config'::text) -> 'core_capabilities'::text) -> 'sharing'::text)) = 'boolean'::text)))) AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'skill_attachments'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skill_attachments'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skill_attachments'::text)) <= 16)
						ELSE false
						END AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'skill_roots'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'skill_roots'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'skill_roots'::text)) <= 8)
						ELSE false
						END AND
						CASE
						WHEN (NOT ((metadata -> 'config'::text) ? 'reference_attachments'::text)) THEN true
						WHEN (jsonb_typeof(((metadata -> 'config'::text) -> 'reference_attachments'::text)) = 'array'::text) THEN (jsonb_array_length(((metadata -> 'config'::text) -> 'reference_attachments'::text)) <= 8)
						ELSE false
						END AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'skill_roots'::text), '[]'::jsonb), 'strict $[*]?(@.type() != "string")'::jsonpath, '{}'::jsonb, true)))) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_task_creation'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_task_creation'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_task_delegation'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_task_delegation'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_memory_write'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_memory_write'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_workspace_retrieval'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_workspace_retrieval'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (NOT ((metadata -> 'config'::text) ? 'allow_cross_conversation_memory'::text)) OR (jsonb_typeof(((metadata -> 'config'::text) -> 'allow_cross_conversation_memory'::text)) = 'boolean'::text)) AND ((kind <> 'agent'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['model'::text, 'instructions'::text, 'tools'::text, 'skills'::text, 'cluster'::text, 'max_steps'::text, 'knowledge_digest'::text, 'core_capabilities'::text, 'skill_attachments'::text, 'skill_roots'::text, 'reference_attachments'::text, 'allow_task_creation'::text, 'allow_task_delegation'::text, 'allow_memory_write'::text, 'allow_workspace_retrieval'::text, 'allow_cross_conversation_memory'::text, 'memory'::text, 'sources'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'tools'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb)) = 'array'::text) AND (NOT jsonb_path_exists(COALESCE(((metadata -> 'config'::text) -> 'skills'::text), '[]'::jsonb), '$[*]?((((@.type() != "object" || !(exists (@."id"))) || @."id".type() != "string") || !(exists (@."version"))) || @."version".type() != "string")'::jsonpath, '{}'::jsonb, true)) AND (jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) = ANY (ARRAY['object'::text, 'null'::text])) AND ((jsonb_typeof(COALESCE(((metadata -> 'config'::text) -> 'cluster'::text), 'null'::jsonb)) <> 'object'::text) OR ((jsonb_typeof(((metadata -> 'config'::text) -> 'cluster'::text)) = 'object'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof((((metadata -> 'config'::text) -> 'cluster'::text) -> 'version'::text)) = 'string'::text))))) AND ((kind <> 'agent'::text) OR ((jsonb_typeof((metadata #> '{config,model}'::text[])) = 'object'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'id'::text)) = 'string'::text) AND (jsonb_typeof(((metadata #> '{config,model}'::text[]) -> 'version'::text)) = 'string'::text)))), false)"#.to_owned() } })
		.atomic(true)
}
