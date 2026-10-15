// reinhardt-migration-source: 1
// ADR 0006 and ADR 0041: a compactor reaches Jev only through OpenRouter's
// System One API with the Tenant's Provider Credential, so its configuration
// names `provider_credential: "openrouter"` and an exact `typesafe/jev-X.Y`
// release instead of a TypeSafe endpoint and an `AIDASH_SECRET_*` key. Like
// every provider model ID, the release fits a Capability Token (256 bytes). Aidash
// is not deployed, so there is no data rewrite: adding the new check fails
// while an old compactor row exists, and such entries must be re-registered
// (or the development database reset). Typed constraint operations suffice.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0025_compactor_provider_keys", "registry")
		.database_only(true)
		.add_dependency("registry", "0024_model_provider_keys")
		.add_operation(Operation::DropConstraintDefinition {
			table: "registry".into(),
			constraint: Constraint::Check {
				name: "registry_compactor_config".into(),
				expression: r#"COALESCE((true AND ((kind <> 'compactor'::text) OR (
						CASE
						WHEN (jsonb_typeof((metadata -> 'config'::text)) = 'object'::text) THEN (((metadata -> 'config'::text) - ARRAY['provider'::text, 'endpoint'::text, 'model'::text, 'credential_env'::text, 'max_request_bytes'::text, 'max_questions'::text, 'max_response_bytes'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof(((metadata -> 'config'::text) -> 'provider'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'provider'::text) = 'typesafe-system-one'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'endpoint'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'endpoint'::text) ~ '^https?://[^/@?#[:space:]]+'::text) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'model'::text)) = 'string'::text) AND (length(btrim(((metadata -> 'config'::text) ->> 'model'::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length(((metadata -> 'config'::text) ->> 'model'::text)) <= 128) AND (jsonb_typeof(((metadata -> 'config'::text) -> 'credential_env'::text)) = 'string'::text) AND (((metadata -> 'config'::text) ->> 'credential_env'::text) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text) AND
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
						END))), false)"#.into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
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
		.atomic(true)
}
