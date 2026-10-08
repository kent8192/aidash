ALTER TABLE semantic_indexes DROP CONSTRAINT semantic_indexes_revision;
ALTER TABLE semantic_indexes ADD CONSTRAINT semantic_indexes_revision CHECK (COALESCE(((revision > 0) AND (revision < '9223372036854775807'::bigint) AND (jsonb_typeof(spec) = 'object'::text) AND
						CASE
						WHEN (jsonb_typeof(spec) = 'object'::text) THEN ((spec - ARRAY['embedding'::text, 'vector'::text, 'enabled'::text, 'auto_context'::text, 'max_sources'::text, 'max_results'::text, 'max_result_tokens'::text, 'max_input_bytes'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((spec -> 'enabled'::text)) = 'boolean'::text) AND (jsonb_typeof((spec -> 'auto_context'::text)) = 'boolean'::text) AND
						CASE
						WHEN ((jsonb_typeof((spec -> 'max_sources'::text)) = 'number'::text) AND (((spec -> 'max_sources'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_sources'::text))::text)::numeric >= (1)::numeric) AND ((((spec -> 'max_sources'::text))::text)::numeric <= (1024)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof((spec -> 'max_results'::text)) = 'number'::text) AND (((spec -> 'max_results'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_results'::text))::text)::numeric >= (1)::numeric) AND ((((spec -> 'max_results'::text))::text)::numeric <= (20)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof((spec -> 'max_result_tokens'::text)) = 'number'::text) AND (((spec -> 'max_result_tokens'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_result_tokens'::text))::text)::numeric >= (128)::numeric) AND ((((spec -> 'max_result_tokens'::text))::text)::numeric <= (32768)::numeric))
						ELSE false
						END AND
						CASE
						WHEN ((jsonb_typeof((spec -> 'max_input_bytes'::text)) = 'number'::text) AND (((spec -> 'max_input_bytes'::text))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec -> 'max_input_bytes'::text))::text)::numeric >= (128)::numeric) AND ((((spec -> 'max_input_bytes'::text))::text)::numeric <= (32768)::numeric))
						ELSE false
						END AND
						CASE
						WHEN (jsonb_typeof((spec -> 'embedding'::text)) = 'object'::text) THEN (((spec -> 'embedding'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text, 'model'::text, 'model_version'::text, 'dimensions'::text]) = '{}'::jsonb)
						ELSE false
						END AND
						CASE
						WHEN (jsonb_typeof((spec -> 'vector'::text)) = 'object'::text) THEN (((spec -> 'vector'::text) - ARRAY['provider'::text, 'endpoint'::text, 'credential_env'::text]) = '{}'::jsonb)
						ELSE false
						END AND ((spec #>> '{embedding,provider}'::text[]) IN ('openai'::text, 'openrouter'::text)) AND public.aidash_valid_http_endpoint((spec #> '{embedding,endpoint}'::text[])) AND (((spec #> '{embedding,credential_env}'::text[]) IS NULL) OR ((spec #> '{embedding,credential_env}'::text[]) = 'null'::jsonb) OR ((jsonb_typeof((spec #> '{embedding,credential_env}'::text[])) = 'string'::text) AND ((spec #>> '{embedding,credential_env}'::text[]) ~ '^AIDASH_SECRET_[A-Z0-9_]*$'::text))) AND ((spec #>> '{vector,provider}'::text[]) = 'postgres'::text) AND ((spec #>> '{vector,endpoint}'::text[]) = 'local'::text) AND (((spec #> '{vector,credential_env}'::text[]) IS NULL) OR ((spec #> '{vector,credential_env}'::text[]) = 'null'::jsonb)) AND ((jsonb_typeof((spec #> '{embedding,model}'::text[])) = 'string'::text) AND (length(btrim((spec #>> '{embedding,model}'::text[]), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length((spec #>> '{embedding,model}'::text[])) <= 256)) AND ((jsonb_typeof((spec #> '{embedding,model_version}'::text[])) = 'string'::text) AND (length(btrim((spec #>> '{embedding,model_version}'::text[]), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (octet_length((spec #>> '{embedding,model_version}'::text[])) <= 128)) AND
						CASE
						WHEN ((jsonb_typeof((spec #> '{embedding,dimensions}'::text[])) = 'number'::text) AND (((spec #> '{embedding,dimensions}'::text[]))::text ~ '^(0|[1-9][0-9]*)$'::text)) THEN (((((spec #> '{embedding,dimensions}'::text[]))::text)::numeric >= (1)::numeric) AND ((((spec #> '{embedding,dimensions}'::text[]))::text)::numeric <= (8192)::numeric))
						ELSE false
						END), false));
