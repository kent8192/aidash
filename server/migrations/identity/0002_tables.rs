// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0002_tables", "identity")
		.database_only(true)
		.add_dependency("federation", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_bundles".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "authorization_bundles_revision_check".to_owned(),
				expression: r#"(revision > 0)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_catalog".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("enabled", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "authorization_catalog_revision_check".to_owned(),
				expression: r#"(revision > 0)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_catalog_history".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("enabled", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("actor", FieldType::Text).with_not_null(true),
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
			name: "authorization_credentials".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("token_hash", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"clock_timestamp()"#.to_owned())),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("revoked_at", FieldType::TimestampTz),
				ColumnDefinition::new("issued_by", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::Check {
					name: "authorization_credentials_check".to_owned(),
					expression: r#"(expires_at > created_at)"#.to_owned(),
				},
				Constraint::Check {
					name: "authorization_credentials_token_hash_check".to_owned(),
					expression: r#"(octet_length(token_hash) = 32)"#.to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_decisions".to_owned(),
			columns: vec![
				ColumnDefinition::new("sequence", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("action", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("resource_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("resource_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("decision", FieldType::Jsonb).with_not_null(true),
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
			name: "authorization_execution".to_owned(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("root_subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject_chain", FieldType::Array(Box::new(FieldType::Text)))
					.with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "authorization_execution_subject_chain_check".to_owned(),
				expression:
					r#"((cardinality(subject_chain) >= 2) AND (cardinality(subject_chain) <= 32))"#
						.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_graph_operator_grants".to_owned(),
			columns: vec![
				ColumnDefinition::new("source_node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_operator", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("enabled", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"clock_timestamp()"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "authorization_graph_operator_grants_revision_check".to_owned(),
				expression: r#"(revision > 0)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_remote_commands".to_owned(),
			columns: vec![
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("request_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_remote_execution".to_owned(),
			columns: vec![
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("admission_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("initial_task", FieldType::Jsonb).with_not_null(true),
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
			name: "authorization_remote_outputs".to_owned(),
			columns: vec![
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("resource_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("resource_id", FieldType::Uuid).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_revisions".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("document", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("actor", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "authorization_revisions_positive".to_owned(),
				expression: r#"COALESCE((revision > 0), false)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_run_outputs".to_owned(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("resource_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("resource_id", FieldType::Uuid).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "authorization_run_reads".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("workspace_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("resource_kind",
				                FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("resource_id", FieldType::Uuid).with_not_null(true)
				            ],
				            constraints: vec![
				                Constraint::Check { name : "authorization_run_reads_resource_kind_check"
				                .to_owned(), expression :
				                r#"(resource_kind = ANY (ARRAY['task'::text, 'artifact'::text, 'message'::text, 'run'::text, 'conversation'::text, 'generation'::text, 'workspace_events'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "authorization_run_registry_reads".to_owned(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_version", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_run_remote_reads".to_owned(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("node_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("entry_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("metadata", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_task_origins".to_owned(),
			columns: vec![
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("source_run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("root_subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject_chain", FieldType::Array(Box::new(FieldType::Text)))
					.with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "authorization_task_origins_subject_chain_check".to_owned(),
				expression:
					r#"((cardinality(subject_chain) >= 2) AND (cardinality(subject_chain) <= 32))"#
						.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_workspaces".to_owned(),
			columns: vec![
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("owner_subject", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "dashboard_execution_origins".to_owned(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("identity_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("mapping_id", FieldType::Uuid).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "dashboard_identities".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("issuer", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("last_valid_at", FieldType::TimestampTz),
				ColumnDefinition::new("disabled_at", FieldType::TimestampTz),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "dashboard_login_transactions".to_owned(),
			columns: vec![
				ColumnDefinition::new("state_hash", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("browser_hash", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("nonce", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("pkce_verifier", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("return_to", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("callback_uri", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "dashboard_logout_tokens".to_owned(),
			columns: vec![
				ColumnDefinition::new("jti_hash", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "dashboard_mappings".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("identity_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("enabled", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "dashboard_operator_grants".to_owned(),
			columns: vec![
				ColumnDefinition::new("identity_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("enabled", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "dashboard_registration_requests".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("identity_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("decided_at", FieldType::TimestampTz),
				ColumnDefinition::new("decided_by", FieldType::Uuid),
				ColumnDefinition::new("decision_actor", FieldType::Text),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "dashboard_sessions".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("token_hash", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("csrf_hash", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("identity_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("provider_sid", FieldType::Text),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("last_activity_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("revoked_at", FieldType::TimestampTz),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0002_configuration.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0002_configuration.sql").to_owned()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
