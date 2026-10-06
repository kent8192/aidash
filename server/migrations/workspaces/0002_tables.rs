// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0002_tables", "workspaces")
		.database_only(true)
		.add_dependency("registry", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
				            name: "artifacts".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("workspace_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("task_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("kind", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("name", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("content", FieldType::Jsonb)
				                .with_not_null(true), ColumnDefinition::new("created_by",
				                FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("idempotency_key", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("created_at",
				                FieldType::TimestampTz).with_not_null(true).with_default(Some(r#"now()"#
				                .to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "artifacts_kind_check".to_owned(), expression
				                :
				                r#"(kind = ANY (ARRAY['text'::text, 'json'::text, 'file_reference'::text, 'code'::text, 'structured_result'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "channel_attachments".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("uploaded_by", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("idempotency_key", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("filename", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("media_type", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("sha256", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("content", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("message_id", FieldType::Uuid),
				ColumnDefinition::new("position".to_owned(), FieldType::Integer)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(Some("0".to_owned()))
					.with_generated(None)
					.with_domain_option(None),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "channel_message_context".to_owned(),
			columns: vec![
				ColumnDefinition::new("message_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("thread_id", FieldType::Uuid),
				ColumnDefinition::new("attachment_digest", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(
						r#"'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'::text"#
							.to_owned(),
					)),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "channel_threads".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("root_message_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("created_by", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "conversations".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("target", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("target_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
				ColumnDefinition::new("created_by", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(r#"'human'::text"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "conversations_target_kind_check".to_owned(),
				expression: r#"(target_kind = ANY (ARRAY['agent'::text, 'cluster'::text]))"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "messages".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("sender", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("content", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("idempotency_key", FieldType::Text),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "task_dependencies".to_owned(),
			columns: vec![
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("dependency_id", FieldType::Uuid).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "tasks".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("workspace_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("title", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("description",
				                FieldType::Text).with_not_null(true), ColumnDefinition::new("status",
				                FieldType::Text).with_not_null(true).with_default(Some(r#"'OPEN'::text"#
				                .to_owned())), ColumnDefinition::new("requirements", FieldType::Jsonb)
				                .with_not_null(true).with_default(Some(r#"'{}'::jsonb"#.to_owned())),
				                ColumnDefinition::new("owner", FieldType::Text),
				                ColumnDefinition::new("created_by", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("dependencies",
				                FieldType::Array(Box::new(FieldType::Uuid))).with_not_null(true)
				                .with_default(Some(r#"'{}'::uuid[]"#.to_owned())),
				                ColumnDefinition::new("parent_id", FieldType::Uuid),
				                ColumnDefinition::new("revision", FieldType::BigInteger)
				                .with_not_null(true).with_default(Some("0".to_owned())),
				                ColumnDefinition::new("creation_key", FieldType::Text),
				                ColumnDefinition::new("completion_key", FieldType::Text),
				                ColumnDefinition::new("created_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some(r#"now()"#.to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "tasks_active_owner".to_owned(), expression :
				                r#"COALESCE(((status <> ALL (ARRAY['CLAIMED'::text, 'RUNNING'::text])) OR (owner IS NOT NULL)), false)"#
				                .to_owned() }, Constraint::Check { name : "tasks_check".to_owned(),
				                expression :
				                r#"(((status = 'OPEN'::text) AND (owner IS NULL)) OR (status <> 'OPEN'::text))"#
				                .to_owned() }, Constraint::Check { name : "tasks_content".to_owned(),
				                expression :
				                r#"COALESCE(((length(btrim(title, E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (length(btrim(description, E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (jsonb_typeof(requirements) = 'object'::text) AND (revision >= 0) AND (revision < '9223372036854775807'::bigint) AND
						CASE
						WHEN (jsonb_typeof(requirements) = 'object'::text) THEN ((requirements - ARRAY['kind'::text, 'query'::text, 'capability'::text, 'language'::text, 'skill'::text, 'tag'::text, 'model'::text]) = '{}'::jsonb)
						ELSE false
						END AND (NOT jsonb_path_exists(requirements, 'strict $.*?(@.type() != "string" && @.type() != "null")'::jsonpath, '{}'::jsonb, true))), false)"#
				                .to_owned() }, Constraint::Check { name : "tasks_no_self_reference"
				                .to_owned(), expression :
				                r#"COALESCE((((parent_id IS NULL) OR (parent_id <> id)) AND (NOT (id = ANY (dependencies))) AND (array_position(dependencies, NULL::uuid) IS NULL)), false)"#
				                .to_owned() }, Constraint::Check { name : "tasks_status_check"
				                .to_owned(), expression :
				                r#"(status = ANY (ARRAY['OPEN'::text, 'CLAIMED'::text, 'RUNNING'::text, 'COMPLETED'::text, 'FAILED'::text, 'BLOCKED'::text, 'CANCELLED'::text, 'ABANDONED'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
				            name: "workspaces".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("title", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("goal", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("state", FieldType::Jsonb).with_not_null(true)
				                .with_default(Some(r#"'{}'::jsonb"#.to_owned())),
				                ColumnDefinition::new("revision", FieldType::BigInteger)
				                .with_not_null(true).with_default(Some("0".to_owned())),
				                ColumnDefinition::new("created_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some(r#"now()"#.to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "workspaces_content".to_owned(), expression :
				                r#"COALESCE(((length(btrim(title, E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (length(btrim(goal, E'\u0009\u000a\u000b\u000c\u000d \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000'::text)) > 0) AND (jsonb_typeof(state) = 'object'::text) AND (revision >= 0)), false)"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
