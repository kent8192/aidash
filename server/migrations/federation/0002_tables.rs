// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0002_tables", "federation")
		.database_only(true)
		.add_dependency("execution", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0002_sequences.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0002_sequences.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "atomic_authority_attempts".to_owned(),
			columns: vec![
				ColumnDefinition::new("transaction_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("node_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "atomic_coordinators".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("manifest", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("decision", FieldType::Text),
				ColumnDefinition::new("visible", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("complete", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("last_error", FieldType::Text),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![
				Constraint::Check {
					name: "atomic_coordinators_check".to_owned(),
					expression:
						r#"((NOT visible) OR ((decision IS NOT NULL) AND (decision = 'COMMIT'::text)))"#
							.to_owned(),
				},
				Constraint::Check {
					name: "atomic_coordinators_check1".to_owned(),
					expression: r#"((NOT complete) OR (decision IS NOT NULL))"#.to_owned(),
				},
				Constraint::Check {
					name: "atomic_coordinators_decision_check".to_owned(),
					expression: r#"(decision = ANY (ARRAY['COMMIT'::text, 'ABORT'::text]))"#.to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "atomic_gate".to_owned(),
			columns: vec![
				ColumnDefinition::new("singleton", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
				ColumnDefinition::new("transaction_id", FieldType::Uuid),
				ColumnDefinition::new("commit_epoch", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "atomic_gate_singleton_check".to_owned(),
				expression: r#"(singleton = true)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "atomic_history".to_owned(),
			columns: vec![
				ColumnDefinition::new("sequence", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some(
						r#"nextval('public.atomic_history_sequence_seq'::regclass)"#.to_owned(),
					)),
				ColumnDefinition::new("transaction_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("role", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("phase", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("detail", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "atomic_history_role_check".to_owned(),
				expression:
					r#"(role = ANY (ARRAY['coordinator'::text, 'participant'::text, 'trust'::text]))"#
						.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "atomic_participants".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("coordinator", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("digest", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("manifest", FieldType::Jsonb)
				                .with_not_null(true), ColumnDefinition::new("phase", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("updated_at",
				                FieldType::TimestampTz).with_not_null(true)
				                .with_default(Some("CURRENT_TIMESTAMP".to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "atomic_participants_phase_check".to_owned(),
				                expression :
				                r#"(phase = ANY (ARRAY['RESERVED'::text, 'PREPARED'::text, 'APPLIED'::text, 'COMMITTED'::text, 'ABORTED'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "atomic_peer_trust".to_owned(),
			columns: vec![
				ColumnDefinition::new("node_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("enabled", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "atomic_preflights".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("binding", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "atomic_subjects".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("binding", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "atomic_votes".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("transaction_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("node_id", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("phase", FieldType::Text)
				                .with_not_null(true).with_default(Some(r#"'PENDING'::text"#.to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "atomic_votes_phase_check".to_owned(),
				                expression :
				                r#"(phase = ANY (ARRAY['PENDING'::text, 'RESERVED'::text, 'PREPARED'::text, 'APPLIED'::text, 'COMMITTED'::text, 'ABORTED'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
					name: "authorization_peer_mapping_history".to_owned(),
					columns: vec![ColumnDefinition::new("sequence", FieldType::BigInteger).with_not_null(true).with_default(Some(r#"nextval('public.authorization_peer_mapping_history_sequence_seq'::regclass)"#.to_owned())), ColumnDefinition::new("source_node", FieldType::Text).with_not_null(true), ColumnDefinition::new("source_tenant", FieldType::Text).with_not_null(true), ColumnDefinition::new("source_subject", FieldType::Text).with_not_null(true), ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true), ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true), ColumnDefinition::new("enabled", FieldType::Boolean).with_not_null(true), ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true), ColumnDefinition::new("actor", FieldType::Text).with_not_null(true), ColumnDefinition::new("updated_at", FieldType::TimestampTz)
							.with_not_null(true)
							.with_default(Some(r#"clock_timestamp()"#.to_owned()))],
					constraints: vec![Constraint::Check {
						name: "authorization_peer_mapping_history_revision_check".to_owned(),
						expression: r#"(revision > 0)"#.to_owned(),
					}],
					without_rowid: None,
					interleave_in_parent: None,
					partition: None,
				})
		.add_operation(Operation::CreateTable {
			name: "authorization_peer_mappings".to_owned(),
			columns: vec![
				ColumnDefinition::new("source_node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("enabled", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("actor", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"clock_timestamp()"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "authorization_peer_mappings_revision_check".to_owned(),
				expression: r#"(revision > 0)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "authorization_remote_admissions".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("source_node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("subject_chain", FieldType::Array(Box::new(FieldType::Text)))
					.with_not_null(true),
				ColumnDefinition::new("description", FieldType::Jsonb).with_not_null(true),
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
			name: "authorization_remote_grant_reads".to_owned(),
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
			name: "authorization_remote_grants".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("node_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("root_subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject_chain", FieldType::Array(Box::new(FieldType::Text)))
					.with_not_null(true),
				ColumnDefinition::new("inspection", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("revoked", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("semantic", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some(r#"'{"mode": "disabled"}'::jsonb"#.to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "delegations".to_owned(),
			columns: vec![
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("node_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("delivered", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
				ColumnDefinition::new("next_attempt_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "peer_events".to_owned(),
			columns: vec![
				ColumnDefinition::new("node_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("event_id", FieldType::Uuid).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "peers".to_owned(),
			columns: vec![
				ColumnDefinition::new("node_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("endpoint", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_env", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("protocol_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("enabled", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "remote_run_message_fences".to_owned(),
			columns: vec![
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("idempotency_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("content", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("input_seq", FieldType::BigInteger),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz),
				ColumnDefinition::new("consumed", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
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
