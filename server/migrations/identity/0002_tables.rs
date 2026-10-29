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
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
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
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
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
				ColumnDefinition::new("mapping_id", FieldType::Uuid),
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
			name: "authorization_remote_commands".to_owned(),
			columns: vec![
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
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
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
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
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
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
				ColumnDefinition::new("actor_mapping_id", FieldType::Uuid),
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
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
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
				                ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
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
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
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
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
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
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
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
			name: "dashboard_deployment_administrator_grants".to_owned(),
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
		// A person's own Approval Mode and Approval Rules (#411, ADR 0076).
		.add_operation(Operation::CreateTable {
			name: "identity_approval_preferences".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("default_mode", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(r#"'manual'::text"#.to_owned())),
				ColumnDefinition::new("answer_deadline_seconds", FieldType::BigInteger),
				ColumnDefinition::new("bypass_accepted_at", FieldType::TimestampTz),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "identity_approval_preferences_values".to_owned(),
				expression: r#"(default_mode IN ('manual', 'auto', 'dont_ask', 'bypass') AND (default_mode <> 'bypass' OR bypass_accepted_at IS NOT NULL) AND (answer_deadline_seconds IS NULL OR answer_deadline_seconds > 0) AND revision > 0)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "identity_approval_rules".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("target", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("effect", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "identity_approval_rules_values".to_owned(),
				expression: r#"(kind IN ('tool_call', 'outbound', 'web_search', 'page_reading', 'generation') AND effect IN ('allow', 'ask', 'deny') AND jsonb_typeof(target) IN ('string', 'object'))"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "identity_thread_approval_modes".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("thread_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("mode", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "identity_thread_approval_modes_values".to_owned(),
				expression: r#"(mode IN ('manual', 'auto', 'dont_ask', 'bypass') AND revision > 0)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// The approval Decider a Tenant chose for the Auto Approval Mode (#412, ADR 0077).
		.add_operation(Operation::CreateTable {
			name: "identity_tenant_approval_settings".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("decider_id", FieldType::Text),
				ColumnDefinition::new("decider_version", FieldType::Text),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![Constraint::Check {
				name: "identity_tenant_approval_settings_values".to_owned(),
				expression: r#"((decider_id IS NULL) = (decider_version IS NULL) AND revision > 0)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "tenants".into(),
			columns: vec![
				ColumnDefinition::new("name", FieldType::Text).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("label", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("personal", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".into())),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("created_by_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_by_identity", FieldType::Uuid).with_not_null(false),
			],
			constraints: vec![
				Constraint::Check {
					name: "tenants_root".into(),
					expression: "length(name) BETWEEN 1 AND 256 AND created_by_kind IN ('identity','deployment_administrator') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND (label IS NULL OR length(label) BETWEEN 1 AND 256)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// Node names are Deployment-wide and never reused, so an inbound request
		// finds the Tenant of its receiving Node without a Deployment Scope and
		// a purged Tenant's names stay taken (#241).
		.add_operation(Operation::CreateTable {
			name: "node_names".into(),
			columns: vec![
				ColumnDefinition::new("name", FieldType::Text).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![Constraint::Unique {
				name: "node_names_name_tenant_key".into(),
				columns: vec!["name".into(), "tenant".into()],
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "nodes".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("name", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("label", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("is_default", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("created_by_kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_by_identity", FieldType::Uuid).with_not_null(false),
			],
			constraints: vec![
				Constraint::Check {
					name: "nodes_creator".into(),
					expression: "created_by_kind IN ('identity','deployment_administrator') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND length(label) BETWEEN 1 AND 64".into(),
				},
			],
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
