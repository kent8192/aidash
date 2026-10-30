// reinhardt-migration-source: 1
// Extend the physical image source and indexing state checks.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0040_working_images", "knowledge")
 .add_dependency("knowledge", "0039_image_index_limits")
 .add_operation(Operation::DropConstraintDefinition { table: "semantic_entries".into(), constraint: Constraint::Check { name: "semantic_entries_counters".into(), expression: r###"COALESCE(((revision > 0) AND (index_revision > 0) AND (attempts >= 0) AND
						CASE (source ->> 'kind'::text)
						WHEN 'memory'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'text'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'text'::text)) = 'string'::text) AND (deleted OR (length(btrim((source ->> 'text'::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0)))
						WHEN 'artifact'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
						WHEN 'message'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
WHEN 'unit'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
						ELSE false
						END), false)"###.into() } })
 .add_operation(Operation::AddConstraintDefinition { table: "semantic_entries".into(), constraint: Constraint::Check { name: "semantic_entries_counters".into(), expression: r###"(COALESCE(((revision > 0) AND (index_revision > 0) AND (attempts >= 0) AND
						CASE (source ->> 'kind'::text)
						WHEN 'memory'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'text'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'text'::text)) = 'string'::text) AND (deleted OR (length(btrim((source ->> 'text'::text), E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0)))
						WHEN 'artifact'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
						WHEN 'message'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
WHEN 'unit'::text THEN (
						CASE
						WHEN (jsonb_typeof(source) = 'object'::text) THEN ((source - ARRAY['kind'::text, 'id'::text]) = '{}'::jsonb)
						ELSE false
						END AND (jsonb_typeof((source -> 'id'::text)) = 'string'::text) AND ((source ->> 'id'::text) ~ '^([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|[0-9a-fA-F]{32}|urn:uuid:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}|\{[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\})$'::text))
						ELSE false
						END), false)) OR (COALESCE((revision > 0 AND index_revision > 0 AND attempts >= 0
AND jsonb_typeof(source) = 'object' AND source - ARRAY['kind','area_id','generation','file_id','path'] = '{}'::jsonb
AND source->>'kind' = 'working_image'
AND CASE WHEN jsonb_typeof(source->'generation') = 'number' AND source->>'generation' ~ '^[1-9][0-9]{0,18}$' THEN (source->>'generation')::numeric <= 9223372036854775807 ELSE false END
AND jsonb_typeof(source->'area_id') = 'string' AND source->>'area_id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
AND jsonb_typeof(source->'file_id') = 'string' AND source->>'file_id' ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
AND jsonb_typeof(source->'path') = 'string' AND octet_length(source->>'path') BETWEEN 1 AND 4096), false))"###.into() } })
 .add_operation(Operation::DropConstraintDefinition { table: "semantic_entries".into(), constraint: Constraint::Check { name: "semantic_entries_state_check".into(), expression: r###"state IN ('PENDING','READY','ERROR','REVOKED','DELETED')"###.into() } })
 .add_operation(Operation::AddConstraintDefinition { table: "semantic_entries".into(), constraint: Constraint::Check { name: "semantic_entries_state_check".into(), expression: r###"state IN ('PENDING','READY','ERROR','REVOKED','DELETED','UNSUPPORTED')"###.into() } })
 .atomic(true).database_only(true)
}
