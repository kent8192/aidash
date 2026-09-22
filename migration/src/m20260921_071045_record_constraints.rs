use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const SEMVER: &str = "version ~ '^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$'";

// Unicode White_Space, matching Rust str::trim without locale-dependent regexes.
const WHITESPACE_SQL: &str = r"U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000'";

// SeaQuery 0.32.7 TableAlterStatement supports column/foreign-key changes, but
// not ADD/DROP CHECK (TableCreateStatement::check only applies to new tables).
// Keep this existing-table DDL exception in SeaORM's transactional migration;
// foreign keys and indexes use SchemaManager's SeaQuery builders.
// https://docs.rs/sea-query/0.32.7/sea_query/table/struct.TableAlterStatement.html
// https://www.sea-ql.org/SeaORM/docs/1.1.x/migration/writing-migration/#using-raw-sql
// Expressions and identifiers below are static, never application input.
const CHECKS: &[(&str, &str, &str)] = &[
	("registry", "registry_semver", SEMVER),
	("packages", "packages_semver", SEMVER),
	(
		"registry",
		"registry_identity",
		"id ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,99}$' AND metadata->'id' = to_jsonb(id) AND metadata->'version' = to_jsonb(version) AND metadata->'kind' = to_jsonb(kind)",
	),
	(
		"registry",
		"registry_metadata_shape",
		// Strict paths preserve array values; silent mode lets shape checks reject missing keys.
		"jsonb_typeof(metadata) = 'object' AND jsonb_typeof(metadata->'name') = 'object' AND metadata->'name' <> '{}'::jsonb AND NOT jsonb_path_exists(metadata, 'strict $.name.* ? (@.type() != \"string\")', '{}'::jsonb, true) AND jsonb_typeof(metadata->'description') = 'object' AND metadata->'description' <> '{}'::jsonb AND NOT jsonb_path_exists(metadata, 'strict $.description.* ? (@.type() != \"string\")', '{}'::jsonb, true) AND jsonb_typeof(metadata->'config') = 'object' AND jsonb_typeof(COALESCE(metadata->'schema', '{}'::jsonb)) = 'object' AND jsonb_typeof(COALESCE(metadata->'capabilities', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'tags', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'languages', '[]'::jsonb)) = 'array' AND jsonb_typeof(COALESCE(metadata->'skills', '[]'::jsonb)) = 'array'",
	),
	(
		"registry",
		"registry_model_config",
		"kind <> 'model' OR (metadata#>>'{config,provider}' = 'openrouter' AND jsonb_typeof(metadata#>'{config,model_id}') = 'string' AND length(btrim(metadata#>>'{config,model_id}', {whitespace})) > 0 AND jsonb_typeof(metadata#>'{config,endpoint}') = 'string' AND length(btrim(metadata#>>'{config,endpoint}', {whitespace})) > 0 AND CASE WHEN jsonb_typeof(metadata#>'{config,context_window}') = 'number' THEN (metadata#>>'{config,context_window}')::numeric >= 2048 AND trunc((metadata#>>'{config,context_window}')::numeric) = (metadata#>>'{config,context_window}')::numeric ELSE false END AND jsonb_typeof(metadata#>'{config,modalities}') = 'array' AND (metadata#>'{config,modalities}') @> '[\"text\"]'::jsonb AND (NOT (metadata->'config' ? 'reasoning_effort') OR metadata#>'{config,reasoning_effort}' = 'null'::jsonb OR metadata#>>'{config,reasoning_effort}' IN ('none','minimal','low','medium','high','xhigh','max')))",
	),
	(
		"registry",
		"registry_agent_config",
		"kind <> 'agent' OR (jsonb_typeof(metadata#>'{config,instructions}') = 'string' AND length(btrim(metadata#>>'{config,instructions}', {whitespace})) > 0 AND CASE WHEN NOT (metadata->'config' ? 'max_steps') THEN true WHEN jsonb_typeof(metadata#>'{config,max_steps}') = 'number' AND (metadata#>>'{config,max_steps}') ~ '^(0|[1-9][0-9]*)$' THEN (metadata#>>'{config,max_steps}')::numeric BETWEEN 1 AND 1000 ELSE false END)",
	),
	("registry", "registry_tool_config", "true"),
	("registry", "registry_cluster_config", "true"),
	("registry", "registry_compactor_config", "true"),
	("registry", "registry_embedding_config", "true"),
	(
		"registry",
		"registry_skill_config",
		"kind <> 'skill' OR (jsonb_typeof(metadata#>'{config,instructions}') = 'string' AND length(btrim(metadata#>>'{config,instructions}', {whitespace})) > 0)",
	),
	(
		"workspaces",
		"workspaces_content",
		"length(btrim(title, {whitespace})) > 0 AND length(btrim(goal, {whitespace})) > 0 AND jsonb_typeof(state) = 'object' AND revision >= 0",
	),
	(
		"tasks",
		"tasks_content",
		"length(btrim(title, {whitespace})) > 0 AND length(btrim(description, {whitespace})) > 0 AND jsonb_typeof(requirements) = 'object' AND revision >= 0",
	),
	(
		"tasks",
		"tasks_no_self_reference",
		"(parent_id IS NULL OR parent_id <> id) AND NOT (id = ANY(dependencies)) AND array_position(dependencies, NULL) IS NULL",
	),
	("runs", "runs_counters", "step >= 0 AND revision >= 0"),
	(
		"runs",
		"runs_lease",
		"(lease_owner IS NULL) = (lease_until IS NULL)",
	),
	(
		"installations",
		"installations_config",
		"jsonb_typeof(config) = 'object'",
	),
	(
		"packages",
		"packages_identity",
		"jsonb_typeof(manifest) = 'object' AND manifest#>'{entity,id}' = to_jsonb(id) AND manifest#>'{entity,version}' = to_jsonb(version)",
	),
	(
		"semantic_indexes",
		"semantic_indexes_revision",
		"revision > 0 AND jsonb_typeof(spec) = 'object'",
	),
	(
		"semantic_entries",
		"semantic_entries_counters",
		"revision > 0 AND index_revision > 0 AND attempts >= 0",
	),
	(
		"semantic_entries",
		"semantic_entries_authority",
		"jsonb_typeof(authority) = 'object' AND authority ? 'credential' AND jsonb_typeof(authority->'credential') IN ('string', 'null') AND authority ? 'tenant' AND jsonb_typeof(authority->'tenant') = 'string' AND authority ? 'subject' AND jsonb_typeof(authority->'subject') = 'string' AND authority ? 'subjects' AND jsonb_typeof(authority->'subjects') = 'array' AND NOT jsonb_path_exists(authority, 'strict $.subjects[*] ? (@.type() != \"string\")', '{}'::jsonb, true) AND (authority->'credential' = 'null'::jsonb OR authority->>'credential' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$')",
	),
	(
		"semantic_run_reads",
		"semantic_run_reads_revision",
		"revision > 0",
	),
	(
		"authorization_revisions",
		"authorization_revisions_positive",
		"revision > 0",
	),
	(
		"generation_policy_history",
		"generation_policy_history_revision",
		"revision > 0",
	),
];

// These expression builders only receive static migration identifiers/paths.
fn object_fields(value: &str, fields: &[&str]) -> String {
	let keys = fields
		.iter()
		.map(|key| format!("'{key}'"))
		.collect::<Vec<_>>()
		.join(",");
	format!(
		"CASE WHEN jsonb_typeof(({value})) = 'object' THEN (({value}) - ARRAY[{keys}]::text[]) = '{{}}'::jsonb ELSE false END"
	)
}
fn string_array(value: &str) -> String {
	format!(
		"jsonb_typeof({value}) = 'array' AND NOT jsonb_path_exists({value}, 'strict $[*] ? (@.type() != \"string\")', '{{}}'::jsonb, true)"
	)
}
fn optional_string(value: &str) -> String {
	format!("jsonb_typeof(COALESCE({value}, 'null'::jsonb)) IN ('string', 'null')")
}
fn semver_value(value: &str) -> String {
	let pattern = "^(0|[1-9][0-9]*)[.](0|[1-9][0-9]*)[.](0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)([.](0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?([+][0-9A-Za-z-]+([.][0-9A-Za-z-]+)*)?$";
	format!(
		"CASE WHEN {value} ~ '{pattern}' THEN split_part({value}, '.', 1)::numeric <= 18446744073709551615 AND split_part({value}, '.', 2)::numeric <= 18446744073709551615 AND split_part(split_part(split_part({value}, '.', 3), '-', 1), '+', 1)::numeric <= 18446744073709551615 ELSE false END"
	)
}
fn unsigned(value: &str) -> String {
	format!(
		"CASE WHEN jsonb_typeof({value}) = 'number' AND ({value})::text ~ '^(0|[1-9][0-9]*)$' THEN ({value})::text::numeric <= 18446744073709551615 ELSE false END"
	)
}
fn unsigned_between(value: &str, min: u64, max: u64) -> String {
	format!(
		"CASE WHEN jsonb_typeof({value}) = 'number' AND ({value})::text ~ '^(0|[1-9][0-9]*)$' THEN ({value})::text::numeric BETWEEN {min} AND {max} ELSE false END"
	)
}
fn tool_config_is_valid_expression(config: &str) -> String {
	let native_hosts = format!(
		"(NOT ({config} ? 'allowed_hosts') OR {})",
		string_array(&format!("{config}->'allowed_hosts'"))
	);
	let native = [
		object_fields(config, &["transport", "operation", "allowed_hosts"]),
		format!(
			"jsonb_typeof({config}->'operation') = 'string' AND {config}->>'operation' IN ('echo', 'http_get')"
		),
		native_hosts,
		format!(
			"({config}->>'operation' <> 'http_get' OR CASE WHEN jsonb_typeof(COALESCE({config}->'allowed_hosts', '[]'::jsonb)) = 'array' THEN jsonb_array_length(COALESCE({config}->'allowed_hosts', '[]'::jsonb)) > 0 ELSE false END)"
		),
	]
	.join(" AND ");
	let http = [
		object_fields(config, &["transport", "endpoint", "credential_env", "replay"]),
		format!(
			"jsonb_typeof({config}->'endpoint') = 'string' AND length(btrim({config}->>'endpoint', {WHITESPACE_SQL})) > 0"
		),
		optional_string(&format!("{config}->'credential_env'")),
		format!(
			"jsonb_typeof({config}->'replay') = 'string' AND {config}->>'replay' IN ('read_only', 'idempotent', 'unsafe')"
		),
	]
	.join(" AND ");
	let mcp = [
		object_fields(
			config,
			&[
				"transport",
				"endpoint",
				"credential_env",
				"tool_name",
				"replay",
				"idempotency_argument",
			],
		),
		format!(
			"jsonb_typeof({config}->'endpoint') = 'string' AND length(btrim({config}->>'endpoint', {WHITESPACE_SQL})) > 0"
		),
		optional_string(&format!("{config}->'credential_env'")),
		format!("jsonb_typeof({config}->'tool_name') = 'string'"),
		format!(
			"jsonb_typeof({config}->'replay') = 'string' AND {config}->>'replay' IN ('read_only', 'idempotent', 'unsafe')"
		),
		optional_string(&format!("{config}->'idempotency_argument'")),
		format!(
			"({config}->>'replay' <> 'idempotent' OR (jsonb_typeof({config}->'idempotency_argument') = 'string' AND length(btrim({config}->>'idempotency_argument', {WHITESPACE_SQL})) > 0))"
		),
	]
	.join(" AND ");
	let agent = [
		object_fields(config, &["transport", "node_id", "agent"]),
		format!(
			"jsonb_typeof({config}->'node_id') = 'string' AND {config}->>'node_id' ~ '^aidash://[A-Za-z0-9-]{{1,100}}$'"
		),
		bounded_entity_ref(&format!("{config}->'agent'")),
	]
	.join(" AND ");
	format!(
		"CASE {config}->>'transport' WHEN 'native' THEN ({native}) WHEN 'http' THEN ({http}) WHEN 'mcp' THEN ({mcp}) WHEN 'agent' THEN ({agent}) ELSE false END"
	)
}
fn entity_ref(value: &str) -> String {
	// EntityRef allows extra keys, but both identity strings are required.
	format!(
		"jsonb_typeof({value}) = 'object' AND jsonb_typeof({value}->'id') = 'string' AND jsonb_typeof({value}->'version') = 'string'"
	)
}
fn bounded_entity_ref(value: &str) -> String {
	format!(
		"{} AND {value}->>'id' ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{{0,99}}$' AND {}",
		entity_ref(value),
		semver_value(&format!("{value}->>'version'"))
	)
}
fn entity_ref_array(value: &str) -> String {
	format!(
		"jsonb_typeof({value}) = 'array' AND NOT jsonb_path_exists({value}, 'lax $[*] ? (@.type() != \"object\" || !exists(@.id) || @.id.type() != \"string\" || !exists(@.version) || @.version.type() != \"string\")', '{{}}'::jsonb, true)"
	)
}
fn checks() -> Vec<(&'static str, &'static str, String)> {
	let mut checks = Vec::new();
	for &(table, name, expression) in CHECKS {
		let mut parts = vec![expression.replace("{whitespace}", WHITESPACE_SQL)];
		match name {
			"registry_semver" | "packages_semver" => {
				parts.push(semver_value("version"));
			}
			"registry_metadata_shape" => {
				parts.push(object_fields(
					"metadata",
					&[
						"id",
						"version",
						"kind",
						"name",
						"description",
						"capabilities",
						"tags",
						"languages",
						"skills",
						"schema",
						"config",
					],
				));
				for field in ["capabilities", "tags", "languages", "skills"] {
					parts.push(string_array(&format!(
						"COALESCE(metadata->'{field}', '[]'::jsonb)"
					)));
				}
			}
			"registry_model_config" => {
				let config = "(metadata->'config')";
				let shape = [
					object_fields(
						config,
						&[
							"provider",
							"model_id",
							"endpoint",
							"credential_env",
							"reasoning_effort",
							"context_window",
							"modalities",
							"cost",
						],
					),
					format!("{config} ? 'cost'"),
					optional_string("metadata#>'{config,credential_env}'"),
					string_array("(metadata#>'{config,modalities}')"),
					unsigned("(metadata#>'{config,context_window}')"),
				];
				// Value is intentionally untyped: cost may contain any JSON value.
				parts.push(format!("kind <> 'model' OR ({})", shape.join(" AND ")));
			}
			"registry_agent_config" => {
				let config = "(metadata->'config')";
				let shape = [
					object_fields(
						config,
						[
							"model",
							"instructions",
							"tools",
							"skills",
							"cluster",
							"max_steps",
						]
						.as_slice(),
					),
					entity_ref_array(&format!("COALESCE({config}->'tools', '[]'::jsonb)")),
					entity_ref_array(&format!("COALESCE({config}->'skills', '[]'::jsonb)")),
					format!(
						"jsonb_typeof(COALESCE({config}->'cluster', 'null'::jsonb)) IN ('object', 'null') AND (jsonb_typeof(COALESCE({config}->'cluster', 'null'::jsonb)) <> 'object' OR {})",
						entity_ref(&format!("{config}->'cluster'"))
					),
				];
				parts.push(format!("kind <> 'agent' OR ({})", shape.join(" AND ")));
				parts.push(format!(
					"kind <> 'agent' OR ({})",
					entity_ref("(metadata#>'{config,model}')")
				));
			}
			"registry_tool_config" => {
				let config = "(metadata->'config')";
				parts.push(format!(
					"kind <> 'tool' OR aidash_tool_config_is_valid({config})"
				));
			}
			"registry_identity" => {
				parts.push(
					"kind <> 'agent' OR octet_length(id) + octet_length(version) <= 139".into(),
				);
			}
			"registry_cluster_config" => {
				let config = "(metadata->'config')";
				let coordinator = format!("{config}->'coordinator'");
				parts.push(format!(
					"kind <> 'cluster' OR ({})",
					object_fields(config, &["coordinator"])
				));
				parts.push(format!(
					"kind <> 'cluster' OR ({})",
					bounded_entity_ref(&coordinator)
				));
			}
			"registry_compactor_config" => {
				let config = "(metadata->'config')";
				let shape = [
					object_fields(
						config,
						&[
							"provider",
							"endpoint",
							"model",
							"credential_env",
							"max_request_bytes",
							"max_questions",
							"max_response_bytes",
						],
					),
					format!(
						"jsonb_typeof({config}->'provider') = 'string' AND {config}->>'provider' = 'typesafe-system-one'"
					),
					format!(
						"jsonb_typeof({config}->'endpoint') = 'string' AND {config}->>'endpoint' ~ '^https?://[^/@?#[:space:]]+'"
					),
					format!(
						"jsonb_typeof({config}->'model') = 'string' AND length(btrim({config}->>'model', {WHITESPACE_SQL})) > 0 AND octet_length({config}->>'model') <= 128"
					),
					format!(
						"jsonb_typeof({config}->'credential_env') = 'string' AND {config}->>'credential_env' ~ '^AIDASH_SECRET_[A-Z0-9_]*$'"
					),
					unsigned_between(&format!("{config}->'max_request_bytes'"), 1024, 1_048_576),
					unsigned_between(&format!("{config}->'max_questions'"), 1, 1024),
					unsigned_between(&format!("{config}->'max_response_bytes'"), 128, 1_048_576),
				];
				parts.push(format!("kind <> 'compactor' OR ({})", shape.join(" AND ")));
			}
			"registry_embedding_config" => {
				let config = "(metadata->'config')";
				let shape = [
					object_fields(
						config,
						&[
							"provider",
							"endpoint",
							"credential_env",
							"model",
							"model_version",
							"dimensions",
						],
					),
					format!(
						"jsonb_typeof({config}->'provider') = 'string' AND {config}->>'provider' = 'openai'"
					),
					format!(
						"jsonb_typeof({config}->'endpoint') = 'string' AND {config}->>'endpoint' ~ '^https?://[^/@?#[:space:]]+'"
					),
					format!(
						"{} AND (jsonb_typeof(COALESCE({config}->'credential_env', 'null'::jsonb)) <> 'string' OR {config}->>'credential_env' ~ '^AIDASH_SECRET_[A-Z0-9_]*$')",
						optional_string(&format!("{config}->'credential_env'"))
					),
					format!(
						"jsonb_typeof({config}->'model') = 'string' AND length(btrim({config}->>'model', {WHITESPACE_SQL})) > 0 AND octet_length({config}->>'model') <= 256"
					),
					format!(
						"jsonb_typeof({config}->'model_version') = 'string' AND length(btrim({config}->>'model_version', {WHITESPACE_SQL})) > 0 AND octet_length({config}->>'model_version') <= 128"
					),
					unsigned_between(&format!("{config}->'dimensions'"), 1, 8192),
				];
				parts.push(format!("kind <> 'embedding' OR ({})", shape.join(" AND ")));
			}
			"tasks_content" => {
				parts.push(object_fields(
					"requirements",
					&[
						"kind",
						"query",
						"capability",
						"language",
						"skill",
						"tag",
						"model",
					],
				));
				parts.push("NOT jsonb_path_exists(requirements, 'strict $.* ? (@.type() != \"string\" && @.type() != \"null\")', '{}'::jsonb, true)".into());
			}
			"runs_counters" => {
				parts.push(
					"jsonb_typeof(context) = 'object' AND jsonb_typeof(pending) = 'object'".into(),
				);
				for field in ["retry_at", "wake_at"] {
					parts.push(format!(
						"NOT (pending ? '{field}') OR aidash_valid_pending_timestamp(pending->'{field}')"
					));
				}
			}
			"semantic_indexes_revision" => {
				parts.push(object_fields(
					"spec",
					&[
						"embedding",
						"vector",
						"enabled",
						"auto_context",
						"max_sources",
						"max_results",
						"max_result_tokens",
						"max_input_bytes",
					],
				));
				for field in ["enabled", "auto_context"] {
					parts.push(format!("jsonb_typeof(spec->'{field}') = 'boolean'"));
				}
				for field in [
					"max_sources",
					"max_results",
					"max_result_tokens",
					"max_input_bytes",
				] {
					parts.push(unsigned(&format!("(spec->'{field}')")));
				}
				parts.push(object_fields(
					"(spec->'embedding')",
					&[
						"provider",
						"endpoint",
						"credential_env",
						"model",
						"model_version",
						"dimensions",
					],
				));
				parts.push(object_fields(
					"(spec->'vector')",
					&["provider", "endpoint", "credential_env"],
				));
				for section in ["embedding", "vector"] {
					for field in ["provider", "endpoint"] {
						parts.push(format!(
							"jsonb_typeof(spec#>'{{{section},{field}}}') = 'string'"
						));
					}
					parts.push(optional_string(&format!(
						"spec#>'{{{section},credential_env}}'"
					)));
				}
				for field in ["model", "model_version"] {
					parts.push(format!(
						"jsonb_typeof(spec#>'{{embedding,{field}}}') = 'string'"
					));
				}
				parts.push(unsigned("(spec#>'{embedding,dimensions}')"));
			}
			"semantic_entries_counters" => {
				let uuid =
					"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}";
				let memory = object_fields("source", &["kind", "text"]);
				let reference = object_fields("source", &["kind", "id"]);
				parts.push(format!("CASE source->>'kind' WHEN 'memory' THEN ({memory} AND jsonb_typeof(source->'text') = 'string') WHEN 'artifact' THEN ({reference} AND jsonb_typeof(source->'id') = 'string' AND source->>'id' ~ '^({uuid}|[0-9a-fA-F]{{32}}|urn:uuid:{uuid}|\\{{{uuid}\\}})$') WHEN 'message' THEN ({reference} AND jsonb_typeof(source->'id') = 'string' AND source->>'id' ~ '^({uuid}|[0-9a-fA-F]{{32}}|urn:uuid:{uuid}|\\{{{uuid}\\}})$') ELSE false END"));
			}
			"packages_identity" => {
				parts.push(object_fields(
					"manifest",
					&["entity", "author", "permissions", "dependencies"],
				));
				parts.push(format!("jsonb_typeof(manifest->'author') = 'string' AND length(btrim(manifest->>'author', {WHITESPACE_SQL})) > 0"));
				parts.push(string_array("manifest->'permissions'"));
				parts.push(entity_ref_array("manifest->'dependencies'"));
				let entity = "(manifest->'entity')";
				parts.push(object_fields(
					entity,
					[
						"id",
						"version",
						"kind",
						"name",
						"description",
						"capabilities",
						"tags",
						"languages",
						"skills",
						"schema",
						"config",
					]
					.as_slice(),
				));
				parts.push(format!("jsonb_typeof({entity}->'id') = 'string' AND {entity}->>'id' ~ '^[a-zA-Z0-9][a-zA-Z0-9._-]{{0,99}}$'"));
				parts.push(format!(
					"jsonb_typeof({entity}->'version') = 'string' AND {}",
					semver_value(&format!("{entity}->>'version'"))
				));
				parts.push(format!("{entity}->>'kind' IN ('agent','tool','skill')"));
				for field in ["name", "description"] {
					parts.push(format!("jsonb_typeof({entity}->'{field}') = 'object' AND {entity}->'{field}' <> '{{}}'::jsonb AND NOT jsonb_path_exists({entity}->'{field}', 'strict $.* ? (@.type() != \"string\")', '{{}}'::jsonb, true)"));
				}
				for field in ["capabilities", "tags", "languages", "skills"] {
					parts.push(string_array(&format!(
						"COALESCE({entity}->'{field}', '[]'::jsonb)"
					)));
				}
				parts.push(format!(
					"jsonb_typeof({entity}->'schema') = 'object' AND jsonb_typeof({entity}->'config') = 'object'"
				));
				let entity_config = format!("{entity}->'config'");
				let entity_tools =
					entity_ref_array(&format!("COALESCE({entity_config}->'tools', '[]'::jsonb)"));
				let entity_skills =
					entity_ref_array(&format!("COALESCE({entity_config}->'skills', '[]'::jsonb)"));
				let agent_config = [
					object_fields(
						&entity_config,
						[
							"model",
							"instructions",
							"tools",
							"skills",
							"cluster",
							"max_steps",
						]
						.as_slice(),
					),
					format!(
						"jsonb_typeof({entity_config}->'model') = 'object' AND jsonb_typeof({entity_config}->'model'->'id') = 'string' AND jsonb_typeof({entity_config}->'model'->'version') = 'string'"
					),
					format!("jsonb_typeof({entity_config}->'instructions') = 'string'"),
					format!(
						"CASE WHEN NOT ({entity_config} ? 'max_steps') THEN true WHEN jsonb_typeof({entity_config}->'max_steps') = 'number' AND ({entity_config}->>'max_steps') ~ '^(0|[1-9][0-9]*)$' THEN ({entity_config}->>'max_steps')::numeric BETWEEN 1 AND 1000 ELSE false END"
					),
					entity_tools,
					entity_skills,
					format!(
						"jsonb_typeof(COALESCE({entity_config}->'cluster', 'null'::jsonb)) IN ('object', 'null') AND (jsonb_typeof(COALESCE({entity_config}->'cluster', 'null'::jsonb)) <> 'object' OR {})",
						entity_ref(&format!("{entity_config}->'cluster'"))
					),
				];
				parts.push(format!(
					"({entity}->>'kind' <> 'agent' OR ({}))",
					agent_config.join(" AND ")
				));
				parts.push(format!(
					"({entity}->>'kind' <> 'skill' OR jsonb_typeof({entity}->'config'->'instructions') = 'string')"
				));
			}
			_ => {}
		}
		checks.push((
			table,
			name,
			parts
				.into_iter()
				.map(|part| format!("({part})"))
				.collect::<Vec<_>>()
				.join(" AND "),
		));
	}
	checks
}

async fn create_sql_validators(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
	let tool_config = tool_config_is_valid_expression("input_value");
	manager
		.get_connection()
		.execute_unprepared(&format!(
			"CREATE FUNCTION aidash_tool_config_is_valid(input_value jsonb) RETURNS boolean LANGUAGE sql IMMUTABLE STRICT AS $$ SELECT ({tool_config}) $$;"
		))
		.await?;
	manager
		.get_connection()
		.execute_unprepared(
			r#"
CREATE FUNCTION aidash_valid_pending_timestamp(input_value jsonb) RETURNS boolean
LANGUAGE plpgsql STABLE AS $function$
DECLARE parsed timestamptz;
BEGIN
    IF jsonb_typeof(input_value) <> 'string' THEN RETURN false; END IF;
    parsed := (input_value #>> '{}')::timestamptz;
    RETURN isfinite(parsed);
EXCEPTION WHEN OTHERS THEN RETURN false;
END
$function$;
"#,
		)
		.await?;
	Ok(())
}

const LINKS: &[(&str, &str, &str, &str)] = &[
	("tasks", "tasks_parent_workspace", "parent_id", "tasks"),
	("artifacts", "artifacts_task_workspace", "task_id", "tasks"),
	(
		"generation_requests",
		"generation_requests_task_workspace",
		"task_id",
		"tasks",
	),
	(
		"human_requests",
		"human_requests_run_workspace",
		"run_id",
		"runs",
	),
];

async fn create_dependencies(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
	manager
		.create_table(
			Table::create()
				.table(Alias::new("task_dependencies"))
				.col(ColumnDef::new(Alias::new("task_id")).uuid().not_null())
				.col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
				.col(
					ColumnDef::new(Alias::new("dependency_id"))
						.uuid()
						.not_null(),
				)
				.primary_key(
					Index::create()
						.col(Alias::new("task_id"))
						.col(Alias::new("dependency_id")),
				)
				.foreign_key(
					ForeignKey::create()
						.name("tasks_dependencies_source_workspace")
						.from_tbl(Alias::new("task_dependencies"))
						.from_col(Alias::new("task_id"))
						.from_col(Alias::new("workspace_id"))
						.to_tbl(Alias::new("tasks"))
						.to_col(Alias::new("id"))
						.to_col(Alias::new("workspace_id"))
						.on_delete(ForeignKeyAction::Cascade),
				)
				.foreign_key(
					ForeignKey::create()
						.name("tasks_dependencies_target_workspace")
						.from_tbl(Alias::new("task_dependencies"))
						.from_col(Alias::new("dependency_id"))
						.from_col(Alias::new("workspace_id"))
						.to_tbl(Alias::new("tasks"))
						.to_col(Alias::new("id"))
						.to_col(Alias::new("workspace_id")),
				)
				.to_owned(),
		)
		.await?;
	manager
		.create_index(
			Index::create()
				.name("task_dependencies_target")
				.table(Alias::new("task_dependencies"))
				.col(Alias::new("dependency_id"))
				.col(Alias::new("workspace_id"))
				.to_owned(),
		)
		.await?;
	// Backfill through SeaQuery; the FKs validate historical array references.
	let select = Query::select()
		.distinct()
		.column(Alias::new("id"))
		.column(Alias::new("workspace_id"))
		.expr(Expr::cust("unnest(dependencies)"))
		.from(Alias::new("tasks"))
		.to_owned();
	let insert = Query::insert()
		.into_table(Alias::new("task_dependencies"))
		.columns(["task_id", "workspace_id", "dependency_id"].map(Alias::new))
		.select_from(select)
		.map_err(|error| DbErr::Custom(error.to_string()))?
		.to_owned();
	manager
		.get_connection()
		.execute(manager.get_database_backend().build(&insert))
		.await?;
	manager
		.get_connection()
		.execute_unprepared(
			r#"
DO $$
BEGIN
    IF EXISTS (
        WITH RECURSIVE walk(start_id, current_id, path, cycle) AS (
            SELECT id, id, ARRAY[id], false FROM tasks
            UNION ALL
            SELECT w.start_id, edge.target_id, w.path || edge.target_id,
                   edge.target_id = ANY(w.path)
            FROM walk w
            CROSS JOIN LATERAL (
                SELECT t.parent_id AS target_id
                FROM tasks t
                WHERE t.id = w.current_id AND t.parent_id IS NOT NULL
                UNION ALL
                SELECT d.dependency_id AS target_id
                FROM task_dependencies d
                WHERE d.task_id = w.current_id
            ) edge
            WHERE NOT w.cycle
        )
        SELECT 1 FROM walk WHERE cycle
    ) THEN
        RAISE EXCEPTION 'task dependency graph contains a cycle'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_dependency_cycle';
    END IF;
END $$;
"#,
		)
		.await?;
	// SeaQuery cannot express PostgreSQL trigger/function DDL. The derived table
	// keeps the array API intact while real FKs protect concurrent writes/deletes.
	manager
		.get_connection()
		.execute_unprepared(
			r#"
CREATE FUNCTION sync_task_dependencies() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        DELETE FROM task_dependencies WHERE task_id = OLD.id;
    END IF;
    INSERT INTO task_dependencies(task_id, workspace_id, dependency_id)
        SELECT DISTINCT NEW.id, NEW.workspace_id, unnest(NEW.dependencies);
    RETURN NEW;
END $$;
CREATE TRIGGER tasks_dependencies_sync AFTER INSERT OR UPDATE OF id, workspace_id, dependencies
    ON tasks FOR EACH ROW EXECUTE FUNCTION sync_task_dependencies();
CREATE FUNCTION guard_task_dependencies() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF pg_trigger_depth() < 2 THEN
        RAISE EXCEPTION 'task_dependencies is maintained by tasks'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_dependencies_managed';
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER task_dependencies_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
    ON task_dependencies FOR EACH STATEMENT EXECUTE FUNCTION guard_task_dependencies();
"#,
		)
		.await?;
	Ok(())
}

async fn create_parent_cycle_guard(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
	// PostgreSQL deliberately disallows cross-row CHECK constraints. A deferred
	// row trigger gives parent updates the same acyclic guarantee as the Rust
	// ancestor walk while allowing the existing parent API to remain unchanged.
	manager
		.get_connection()
		.execute_unprepared(
			r#"
DO $$
BEGIN
    IF EXISTS (
        WITH RECURSIVE walk(start_id, current_id, parent_id, path, cycle) AS (
            SELECT id, id, parent_id, ARRAY[id], false
            FROM tasks
            WHERE parent_id IS NOT NULL
            UNION ALL
            SELECT w.start_id, t.id, t.parent_id, w.path || t.id, t.id = ANY(w.path)
            FROM walk w
            JOIN tasks t ON t.id = w.parent_id
            WHERE NOT w.cycle
        )
        SELECT 1 FROM walk WHERE cycle
    ) THEN
        RAISE EXCEPTION 'task parent hierarchy contains a cycle'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_parent_cycle';
    END IF;
END $$;
CREATE FUNCTION guard_task_parent_cycle() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- The statement trigger takes the shared lock before deferred-check
    -- snapshots are established; keep this acquisition as a defensive guard.
    -- Scope serialization to this schema's task hierarchy. Isolated tenant/test
    -- schemas share a database but must not block one another's graph writes.
    PERFORM pg_advisory_xact_lock(70721021, hashtext(TG_TABLE_SCHEMA));
    IF EXISTS (
        WITH RECURSIVE walk(current_id, parent_id, path, cycle) AS (
            SELECT id, parent_id, ARRAY[id], false
            FROM tasks
            WHERE id = NEW.id
            UNION ALL
            SELECT t.id, t.parent_id, w.path || t.id, t.id = ANY(w.path)
            FROM walk w
            JOIN tasks t ON t.id = w.parent_id
            WHERE NOT w.cycle
        )
        SELECT 1 FROM walk WHERE cycle
    ) THEN
        RAISE EXCEPTION 'task parent hierarchy contains a cycle'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_parent_cycle';
    END IF;
    RETURN NEW;
END $$;
CREATE CONSTRAINT TRIGGER tasks_parent_cycle_guard
    AFTER INSERT OR UPDATE OF id, parent_id, workspace_id ON tasks
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION guard_task_parent_cycle();
CREATE FUNCTION lock_task_hierarchy_before_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(70721021, hashtext(TG_TABLE_SCHEMA));
    RETURN NULL;
END $$;
CREATE TRIGGER tasks_hierarchy_serialize
    BEFORE INSERT OR UPDATE OF id, parent_id, workspace_id, dependencies ON tasks
    FOR EACH STATEMENT EXECUTE FUNCTION lock_task_hierarchy_before_change();
"#,
		)
		.await?;
	Ok(())
}

async fn create_task_dependency_cycle_guard(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
	manager
		.get_connection()
		.execute_unprepared(
			r#"
CREATE FUNCTION guard_task_dependency_cycle() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        WITH RECURSIVE walk(current_id, path, cycle) AS (
            SELECT NEW.id, ARRAY[NEW.id], false
            UNION ALL
            SELECT edge.target_id, w.path || edge.target_id,
                   edge.target_id = ANY(w.path)
            FROM walk w
            CROSS JOIN LATERAL (
                SELECT t.parent_id AS target_id
                FROM tasks t
                WHERE t.id = w.current_id AND t.parent_id IS NOT NULL
                UNION ALL
                SELECT d.dependency_id AS target_id
                FROM task_dependencies d
                WHERE d.task_id = w.current_id
            ) edge
            WHERE NOT w.cycle
        )
        SELECT 1 FROM walk WHERE cycle
    ) THEN
        RAISE EXCEPTION 'task dependency graph contains a cycle'
            USING ERRCODE = '23514', CONSTRAINT = 'tasks_dependency_cycle';
    END IF;
    RETURN NEW;
END $$;
CREATE CONSTRAINT TRIGGER zz_tasks_dependency_cycle_guard
    AFTER INSERT OR UPDATE OF id, parent_id, dependencies ON tasks
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION guard_task_dependency_cycle();
"#,
		)
		.await?;
	Ok(())
}

async fn create_installation_guard(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
	// Installation overrides are merged with the referenced registry config at
	// read time. Validate each override field against the already-valid base row
	// so direct writes cannot create an invalid effective typed configuration.
	manager
		.get_connection()
		.execute_unprepared(
			r#"
CREATE FUNCTION guard_installation_config() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    target_kind text;
    target_config jsonb;
BEGIN
    SELECT kind, metadata->'config' INTO target_kind, target_config
    FROM registry
    WHERE id = NEW.id AND version = NEW.version;

    IF target_kind = 'model' AND NOT COALESCE(
        (NEW.config - ARRAY['provider','model_id','endpoint','credential_env','reasoning_effort','context_window','modalities','cost']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'provider' OR (jsonb_typeof(NEW.config->'provider') = 'string' AND NEW.config->>'provider' = 'openrouter'))
        AND (NOT NEW.config ? 'model_id' OR (jsonb_typeof(NEW.config->'model_id') = 'string' AND length(btrim(NEW.config->>'model_id', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
        AND (NOT NEW.config ? 'endpoint' OR (jsonb_typeof(NEW.config->'endpoint') = 'string' AND length(btrim(NEW.config->>'endpoint', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
        AND (NOT NEW.config ? 'credential_env' OR jsonb_typeof(NEW.config->'credential_env') IN ('string','null'))
        AND (NOT NEW.config ? 'reasoning_effort' OR NEW.config->'reasoning_effort' = 'null'::jsonb OR NEW.config->>'reasoning_effort' IN ('none','minimal','low','medium','high','xhigh','max'))
        AND (NOT NEW.config ? 'context_window' OR (jsonb_typeof(NEW.config->'context_window') = 'number' AND NEW.config->>'context_window' ~ '^(0|[1-9][0-9]*)$' AND (NEW.config->>'context_window')::numeric >= 2048 AND (NEW.config->>'context_window')::numeric <= 18446744073709551615))
        AND (NOT NEW.config ? 'modalities' OR (jsonb_typeof(NEW.config->'modalities') = 'array' AND NEW.config->'modalities' @> '["text"]'::jsonb AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(NEW.config->'modalities') AS item WHERE jsonb_typeof(item) <> 'string')))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid model configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'agent' AND NOT COALESCE(
        (NEW.config - ARRAY['model','instructions','tools','skills','cluster','max_steps']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'model' OR (jsonb_typeof(NEW.config->'model') = 'object' AND jsonb_typeof(NEW.config->'model'->'id') = 'string' AND jsonb_typeof(NEW.config->'model'->'version') = 'string'))
        AND (NOT NEW.config ? 'instructions' OR (jsonb_typeof(NEW.config->'instructions') = 'string' AND length(btrim(NEW.config->>'instructions', U&'\0009\000A\000B\000C\000D\0020\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000')) > 0))
        AND (NOT NEW.config ? 'tools' OR (jsonb_typeof(NEW.config->'tools') = 'array' AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(NEW.config->'tools') AS item WHERE jsonb_typeof(item) <> 'object' OR jsonb_typeof(item->'id') <> 'string' OR jsonb_typeof(item->'version') <> 'string')))
        AND (NOT NEW.config ? 'skills' OR (jsonb_typeof(NEW.config->'skills') = 'array' AND NOT EXISTS (SELECT 1 FROM jsonb_array_elements(NEW.config->'skills') AS item WHERE jsonb_typeof(item) <> 'object' OR jsonb_typeof(item->'id') <> 'string' OR jsonb_typeof(item->'version') <> 'string')))
        AND (NOT NEW.config ? 'cluster' OR NEW.config->'cluster' = 'null'::jsonb OR (jsonb_typeof(NEW.config->'cluster') = 'object' AND jsonb_typeof(NEW.config->'cluster'->'id') = 'string' AND jsonb_typeof(NEW.config->'cluster'->'version') = 'string'))
        AND (NOT NEW.config ? 'max_steps' OR (jsonb_typeof(NEW.config->'max_steps') = 'number' AND NEW.config->>'max_steps' ~ '^(0|[1-9][0-9]*)$' AND (NEW.config->>'max_steps')::numeric BETWEEN 1 AND 1000))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid agent configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'cluster' AND NOT COALESCE(
        (NEW.config - ARRAY['coordinator']::text[]) = '{}'::jsonb
        AND (NOT NEW.config ? 'coordinator' OR (jsonb_typeof(NEW.config->'coordinator') = 'object' AND jsonb_typeof(NEW.config->'coordinator'->'id') = 'string' AND jsonb_typeof(NEW.config->'coordinator'->'version') = 'string'))
    , false) THEN
        RAISE EXCEPTION 'installation override is not a valid cluster configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    IF target_kind = 'tool' AND NOT COALESCE(
        aidash_tool_config_is_valid(target_config || NEW.config), false
    ) THEN
        RAISE EXCEPTION 'installation override is not a valid tool configuration'
            USING ERRCODE = '23514', CONSTRAINT = 'installations_config';
    END IF;

    RETURN NEW;
END $$;
CREATE TRIGGER installations_config_guard
    BEFORE INSERT OR UPDATE OF id, version, config ON installations
    FOR EACH ROW EXECUTE FUNCTION guard_installation_config();
"#,
		)
		.await?;
	// Fire the trigger for historical rows through SeaQuery so existing
	// installation overrides are validated during the upgrade.
	let backfill = Query::update()
		.table(Alias::new("installations"))
		.value(Alias::new("config"), Expr::col(Alias::new("config")))
		.to_owned();
	manager
		.get_connection()
		.execute(manager.get_database_backend().build(&backfill))
		.await?;
	Ok(())
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		create_sql_validators(manager).await?;
		for (table, name, expression) in checks() {
			// PostgreSQL CHECK alone accepts NULL; missing required JSON keys must fail.
			manager
				.get_connection()
				.execute_unprepared(&format!(
					"ALTER TABLE \"{table}\" ADD CONSTRAINT \"{name}\" CHECK (COALESCE(({expression}), false))"
				))
				.await?;
		}
		for table in ["tasks", "runs"] {
			manager
				.create_index(
					Index::create()
						.name(format!("{table}_id_workspace_unique"))
						.table(Alias::new(table))
						.col(Alias::new("id"))
						.col(Alias::new("workspace_id"))
						.unique()
						.to_owned(),
				)
				.await?;
		}
		for (table, name, column, target) in LINKS {
			manager
				.create_foreign_key(
					ForeignKey::create()
						.name(*name)
						.from_tbl(Alias::new(*table))
						.from_col(Alias::new(*column))
						.from_col(Alias::new("workspace_id"))
						.to_tbl(Alias::new(*target))
						.to_col(Alias::new("id"))
						.to_col(Alias::new("workspace_id"))
						.to_owned(),
				)
				.await?;
		}
		create_installation_guard(manager).await?;
		create_dependencies(manager).await?;
		create_parent_cycle_guard(manager).await?;
		create_task_dependency_cycle_guard(manager).await?;
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.get_connection()
			.execute_unprepared(
				"DROP TRIGGER installations_config_guard ON installations; DROP FUNCTION guard_installation_config(); DROP TRIGGER zz_tasks_dependency_cycle_guard ON tasks; DROP FUNCTION guard_task_dependency_cycle(); DROP TRIGGER tasks_parent_cycle_guard ON tasks; DROP FUNCTION guard_task_parent_cycle(); DROP TRIGGER tasks_hierarchy_serialize ON tasks; DROP FUNCTION lock_task_hierarchy_before_change();",
			)
			.await?;
		// Matching DDL exception: SeaQuery has no trigger/function drop builders.
		manager
			.get_connection()
			.execute_unprepared(
				"DROP TRIGGER tasks_dependencies_sync ON tasks; DROP FUNCTION sync_task_dependencies();",
			)
			.await?;
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("task_dependencies"))
					.to_owned(),
			)
			.await?;
		manager
			.get_connection()
			.execute_unprepared("DROP FUNCTION guard_task_dependencies()")
			.await?;
		for (table, name, _, _) in LINKS.iter().rev() {
			manager
				.drop_foreign_key(
					ForeignKey::drop()
						.table(Alias::new(*table))
						.name(*name)
						.to_owned(),
				)
				.await?;
		}
		for table in ["runs", "tasks"] {
			manager
				.drop_index(
					Index::drop()
						.name(format!("{table}_id_workspace_unique"))
						.table(Alias::new(table))
						.to_owned(),
				)
				.await?;
		}
		for (table, name, _) in CHECKS.iter().rev() {
			manager
				.get_connection()
				.execute_unprepared(&format!(
					"ALTER TABLE \"{table}\" DROP CONSTRAINT \"{name}\""
				))
				.await?;
		}
		manager
			.get_connection()
			.execute_unprepared(
				"DROP FUNCTION aidash_valid_pending_timestamp(jsonb); DROP FUNCTION aidash_tool_config_is_valid(jsonb);",
			)
			.await?;
		Ok(())
	}
}
