// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0002_tables", "execution")
		.database_only(true)
		.add_dependency("workspaces", "0001_functions")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0002_sequences.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/0002_sequences.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "activation_quarantine".to_owned(),
			columns: vec![
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("stream_sequence", FieldType::BigInteger),
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
			name: "core_areas".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("home_node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("owner", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("thread_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("generation", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
				ColumnDefinition::new("epoch", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
				ColumnDefinition::new("next_sequence", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
				ColumnDefinition::new("state", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(r#"'active'::text"#.to_owned())),
				ColumnDefinition::new("manifest", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some(r#"'[]'::jsonb"#.to_owned())),
				ColumnDefinition::new("constraints", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some(r#"'[]'::jsonb"#.to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_objects".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
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
			name: "core_operations".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("principal", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("request_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("epoch", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("generation", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("policy_revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("subjects", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("input", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("runner_instance", FieldType::Text),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
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
			name: "core_quotas".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("used_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_records".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("owner", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
				ColumnDefinition::new("data", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_requests".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("principal", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("key", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_runs".to_owned(),
			columns: vec![
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("sequence", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("initialized", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("generation", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_task_sessions".to_owned(),
			columns: vec![
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("thread_id", FieldType::Uuid).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "events".to_owned(),
			columns: vec![
				ColumnDefinition::new("sequence", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("node_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("data", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("published_at", FieldType::TimestampTz),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
				ColumnDefinition::new("next_attempt_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
				ColumnDefinition::new("publish_error", FieldType::Text),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "generation_budgets".to_owned(),
			columns: vec![
				ColumnDefinition::new("request_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("token_limit", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("used_tokens", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
				ColumnDefinition::new("compaction_call_limit", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
				ColumnDefinition::new("compaction_calls", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
				ColumnDefinition::new("embedding_call_limit", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
				ColumnDefinition::new("embedding_calls", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
			],
			constraints: vec![
				Constraint::Check {
					name: "embedding_calls_bounded".to_owned(),
					expression: r#"(embedding_calls <= embedding_call_limit)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_budgets_check".to_owned(),
					expression: r#"((used_tokens >= 0) AND (used_tokens <= token_limit))"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_budgets_check1".to_owned(),
					expression:
						r#"((compaction_calls >= 0) AND (compaction_calls <= compaction_call_limit))"#
							.to_owned(),
				},
				Constraint::Check {
					name: "generation_budgets_compaction_call_limit_check".to_owned(),
					expression: r#"(compaction_call_limit >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_budgets_embedding_call_limit_check".to_owned(),
					expression: r#"(embedding_call_limit >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_budgets_embedding_calls_check".to_owned(),
					expression: r#"(embedding_calls >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_budgets_token_limit_check".to_owned(),
					expression: r#"(token_limit > 0)"#.to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "generation_compaction_usage".to_owned(),
			columns: vec![
				ColumnDefinition::new("request_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("attempt_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("provider_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("provider_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("request_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("questions", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"clock_timestamp()"#.to_owned())),
			],
			constraints: vec![
				Constraint::Check {
					name: "generation_compaction_usage_questions_check".to_owned(),
					expression: r#"(questions > 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_compaction_usage_request_bytes_check".to_owned(),
					expression: r#"(request_bytes > 0)"#.to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "generation_embedding_usage".to_owned(),
			columns: vec![
				ColumnDefinition::new("request_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("attempt_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid),
				ColumnDefinition::new("entry_id", FieldType::Uuid),
				ColumnDefinition::new("purpose", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("provider_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("provider_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("request_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("reserved_tokens", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("reported_tokens", FieldType::BigInteger),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"clock_timestamp()"#.to_owned())),
			],
			constraints: vec![
				Constraint::Check {
					name: "generation_embedding_usage_purpose_check".to_owned(),
					expression: r#"(purpose = ANY (ARRAY['query'::text, 'index'::text]))"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_embedding_usage_reported_tokens_check".to_owned(),
					expression: r#"(reported_tokens >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_embedding_usage_request_bytes_check".to_owned(),
					expression: r#"(request_bytes > 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_embedding_usage_reserved_tokens_check".to_owned(),
					expression: r#"(reserved_tokens > 0)"#.to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "generation_history".to_owned(),
			columns: vec![
				ColumnDefinition::new("sequence", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("request_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("actor", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
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
			name: "generation_policies".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("spec", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("generated_count", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
				ColumnDefinition::new("allocated_tokens", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
				ColumnDefinition::new("allocated_compaction_calls", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
				ColumnDefinition::new("allocated_embedding_calls", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
			],
			constraints: vec![
				Constraint::Check {
					name: "generation_policies_allocated_compaction_calls_check".to_owned(),
					expression: r#"(allocated_compaction_calls >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_policies_allocated_embedding_calls_check".to_owned(),
					expression: r#"(allocated_embedding_calls >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_policies_allocated_tokens_check".to_owned(),
					expression: r#"(allocated_tokens >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_policies_generated_count_check".to_owned(),
					expression: r#"(generated_count >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_policies_revision_check".to_owned(),
					expression: r#"(revision > 0)"#.to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "generation_policy_history".to_owned(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("policy_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("spec", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("actor", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "generation_policy_history_revision".to_owned(),
				expression: r#"COALESCE((revision > 0), false)"#.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "generation_remote_dispatches".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("attempt_id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("usage", FieldType::Jsonb).with_not_null(true),
				                ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("peer_node", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("boundary", FieldType::Jsonb).with_not_null(true),
				                ColumnDefinition::new("state", FieldType::Text).with_not_null(true)
				                .with_default(Some(r#"'PREPARING'::text"#.to_owned())),
				                ColumnDefinition::new("reservations", FieldType::Jsonb)
				                .with_not_null(true).with_default(Some(r#"'[]'::jsonb"#.to_owned())),
				                ColumnDefinition::new("finalization", FieldType::Jsonb),
				                ColumnDefinition::new("peer_finalized", FieldType::Boolean)
				                .with_not_null(true).with_default(Some("false".to_owned())),
				                ColumnDefinition::new("created_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "generation_remote_dispatches_state_check"
				                .to_owned(), expression :
				                r#"(state = ANY (ARRAY['PREPARING'::text, 'DISPATCHED'::text, 'SETTLED'::text, 'ABORTED'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "generation_remote_finalizations".to_owned(),
			columns: vec![
				ColumnDefinition::new("attempt_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Jsonb),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "generation_remote_intents".to_owned(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("root_subject", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("subject_chain", FieldType::Array(Box::new(FieldType::Text)))
					.with_not_null(true),
				ColumnDefinition::new("binding", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("cancelled", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("cancel_delivered", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
				ColumnDefinition::new("cancel_retry_at", FieldType::TimestampTz),
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
				            name: "generation_remote_usage".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("request_id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("attempt_id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("operation_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("dispatcher_node",
				                FieldType::Text).with_not_null(true), ColumnDefinition::new("grant_id",
				                FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("admission_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("purpose", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("digest", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("reserved_tokens",
				                FieldType::BigInteger).with_not_null(true),
				                ColumnDefinition::new("reported_tokens", FieldType::BigInteger),
				                ColumnDefinition::new("state", FieldType::Text).with_not_null(true)
				                .with_default(Some(r#"'RESERVED'::text"#.to_owned())),
				                ColumnDefinition::new("created_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".to_owned()))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "generation_remote_usage_purpose_check"
				                .to_owned(), expression :
				                r#"(purpose = ANY (ARRAY['embedding'::text, 'inference'::text, 'compaction'::text]))"#
				                .to_owned() }, Constraint::Check { name :
				                "generation_remote_usage_reserved_tokens_check".to_owned(), expression :
				                r#"(reserved_tokens >= 0)"#.to_owned() }, Constraint::Check { name :
				                "generation_remote_usage_state_check".to_owned(), expression :
				                r#"(state = ANY (ARRAY['RESERVED'::text, 'SETTLED'::text, 'RELEASED'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
				            name: "generation_requests".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("policy_id", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("policy_revision", FieldType::BigInteger)
				                .with_not_null(true), ColumnDefinition::new("task_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("workspace_id",
				                FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("credential_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("root_subject",
				                FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("subject_chain",
				                FieldType::Array(Box::new(FieldType::Text))).with_not_null(true),
				                ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				                ColumnDefinition::new("agent_version", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("definition",
				                FieldType::Jsonb).with_not_null(true), ColumnDefinition::new("status",
				                FieldType::Text).with_not_null(true), ColumnDefinition::new("reason",
				                FieldType::Text).with_not_null(true), ColumnDefinition::new("depth",
				                FieldType::Integer).with_not_null(true),
				                ColumnDefinition::new("token_limit", FieldType::BigInteger)
				                .with_not_null(true), ColumnDefinition::new("quota_released",
				                FieldType::Boolean).with_not_null(true).with_default(Some("false"
				                .to_owned())), ColumnDefinition::new("expires_at",
				                FieldType::TimestampTz).with_not_null(true),
				                ColumnDefinition::new("created_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some(r#"now()"#.to_owned())),
				                ColumnDefinition::new("retired_catalog_revision", FieldType::BigInteger),
				                ColumnDefinition::new("home_node", FieldType::Text).with_not_null(true)
				                .with_default(Some(r#"''::text"#.to_owned())),
				                ColumnDefinition::new("foreign_intent", FieldType::Jsonb),
				                ColumnDefinition::new("prepared", FieldType::Boolean).with_not_null(true)
				                .with_default(Some("false".to_owned())),
				                ColumnDefinition::new("grant_id", FieldType::Uuid),
				                ColumnDefinition::new("admission_id", FieldType::Uuid),
				                ColumnDefinition::new("local_task_id", FieldType::Uuid)
				                .with_generated(Some(GeneratedColumnDefinition::raw_sql(r#"
						CASE
						WHEN (home_node = ''::text) THEN task_id
						ELSE NULL::uuid
						END"#,
				                GeneratedStorage::Stored))), ColumnDefinition::new("local_workspace_id",
				                FieldType::Uuid)
				                .with_generated(Some(GeneratedColumnDefinition::raw_sql(r#"
						CASE
						WHEN (home_node = ''::text) THEN workspace_id
						ELSE NULL::uuid
						END"#,
				                GeneratedStorage::Stored)))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "generation_foreign_binding".to_owned(),
				                expression :
				                r#"(((home_node = ''::text) AND (foreign_intent IS NULL) AND (grant_id IS NULL) AND (admission_id IS NULL)) OR ((home_node <> ''::text) AND (foreign_intent IS NOT NULL)))"#
				                .to_owned() }, Constraint::Check { name :
				                "generation_requests_depth_check".to_owned(), expression :
				                r#"((depth >= 1) AND (depth <= 31))"#.to_owned() }, Constraint::Check {
				                name : "generation_requests_status_check".to_owned(), expression :
				                r#"(status = ANY (ARRAY['PENDING_APPROVAL'::text, 'QUEUED'::text, 'ACTIVE'::text, 'COMPLETED'::text, 'DENIED'::text, 'STOPPED'::text, 'EXPIRED'::text, 'FAILED'::text, 'DELETED'::text]))"#
				                .to_owned() }, Constraint::Check { name :
				                "generation_requests_subject_chain_check".to_owned(), expression :
				                r#"((cardinality(subject_chain) >= 1) AND (cardinality(subject_chain) <= 31))"#
				                .to_owned() }, Constraint::Check { name :
				                "generation_requests_token_limit_check".to_owned(), expression :
				                r#"(token_limit > 0)"#.to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "generation_usage".to_owned(),
			columns: vec![
				ColumnDefinition::new("request_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("attempt_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("reserved_tokens", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("reported_tokens", FieldType::BigInteger),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![
				Constraint::Check {
					name: "generation_usage_reported_tokens_check".to_owned(),
					expression: r#"(reported_tokens >= 0)"#.to_owned(),
				},
				Constraint::Check {
					name: "generation_usage_reserved_tokens_check".to_owned(),
					expression: r#"(reserved_tokens > 0)"#.to_owned(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "human_requests".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("workspace_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("run_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("kind", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("prompt", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("response",
				                FieldType::Jsonb), ColumnDefinition::new("request_key", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("created_at",
				                FieldType::TimestampTz).with_not_null(true).with_default(Some(r#"now()"#
				                .to_owned())), ColumnDefinition::new("answered_by", FieldType::Text)
				            ],
				            constraints: vec![
				                Constraint::Check { name : "human_requests_kind_check".to_owned(),
				                expression :
				                r#"(kind = ANY (ARRAY['QUESTION'::text, 'APPROVAL_REQUIRED'::text, 'CONFIRMATION'::text, 'INFORMATION_REQUEST'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "inbox".to_owned(),
			columns: vec![
				ColumnDefinition::new("event_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("received_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "invocations".to_owned(),
			columns: vec![
				ColumnDefinition::new("idempotency_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("tool", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("input", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Jsonb),
				ColumnDefinition::new("replay_safe", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some(r#"now()"#.to_owned())),
			],
			constraints: vec![Constraint::Check {
				name: "invocations_status_check".to_owned(),
				expression:
					r#"(status = ANY (ARRAY['STARTED'::text, 'COMPLETED'::text, 'UNCERTAIN'::text]))"#
						.to_owned(),
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "memory".to_owned(),
			columns: vec![
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("data", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some(r#"'{}'::jsonb"#.to_owned())),
				ColumnDefinition::new("home_node", FieldType::Text)
					.with_not_null(true)
					.with_default(Some(r#"''::text"#.to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "run_activations".to_owned(),
				            columns: vec![ColumnDefinition::new("generation", FieldType::BigInteger)
				                .with_not_null(true).with_default(Some(r#"nextval('public.run_activations_generation_seq'::regclass)"#.to_owned())), ColumnDefinition::new("id", FieldType::Uuid)
				                .with_not_null(true).with_default(Some(r#"gen_random_uuid()"#
				                .to_owned())), ColumnDefinition::new("run_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("run_revision",
				                FieldType::BigInteger).with_not_null(true), ColumnDefinition::new("reason", FieldType::Text).with_not_null(true), ColumnDefinition::new("state", FieldType::Text).with_not_null(true)
				                .with_default(Some(r#"'pending'::text"#.to_owned())), ColumnDefinition::new("due_at", FieldType::TimestampTz)
				                .with_default(Some("CURRENT_TIMESTAMP".to_owned())), ColumnDefinition::new("publication_epoch", FieldType::BigInteger)
				                .with_not_null(true).with_default(Some("0".to_owned())), ColumnDefinition::new("publish_token", FieldType::Uuid), ColumnDefinition::new("publish_until", FieldType::TimestampTz), ColumnDefinition::new("published_at", FieldType::TimestampTz), ColumnDefinition::new("lease_token", FieldType::Uuid), ColumnDefinition::new("claimed_at", FieldType::TimestampTz), ColumnDefinition::new("claim_source", FieldType::Text), ColumnDefinition::new("worker_pid", FieldType::BigInteger), ColumnDefinition::new("disposition", FieldType::Text), ColumnDefinition::new("created_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some("CURRENT_TIMESTAMP".to_owned()))],
				            constraints: vec![
				                Constraint::Check { name : "run_activations_state_check".to_owned(),
				                expression :
				                r#"(state = ANY (ARRAY['pending'::text, 'claimed'::text, 'deferred'::text, 'settled'::text]))"#
				                .to_owned() }
				            ],
				            without_rowid: None,
				            interleave_in_parent: None,
				            partition: None,
				        })
		.add_operation(Operation::CreateTable {
			name: "run_inputs".to_owned(),
			columns: vec![
				ColumnDefinition::new("seq", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some(
						r#"nextval('public.run_inputs_seq_seq'::regclass)"#.to_owned(),
					)),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("sender", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("content", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("message_id", FieldType::Uuid),
				ColumnDefinition::new("idempotency_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("delivery_retry_at", FieldType::TimestampTz),
				ColumnDefinition::new("reference_only", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
				            name: "runs".to_owned(),
				            columns: vec![
				                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("task_id", FieldType::Uuid).with_not_null(true),
				                ColumnDefinition::new("workspace_id", FieldType::Uuid)
				                .with_not_null(true), ColumnDefinition::new("home_node", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("agent_id", FieldType::Text)
				                .with_not_null(true), ColumnDefinition::new("agent_version",
				                FieldType::Text).with_not_null(true), ColumnDefinition::new("phase",
				                FieldType::Text).with_not_null(true).with_default(Some(r#"'READY'::text"#
				                .to_owned())), ColumnDefinition::new("control", FieldType::Text)
				                .with_not_null(true).with_default(Some(r#"'ACTIVE'::text"#.to_owned())),
				                ColumnDefinition::new("context", FieldType::Jsonb).with_not_null(true)
				                .with_default(Some(r#"'{"usage": null, "history": [], "summary": "", "compactions": 0, "media_inferred_seq": 0, "run_message_summary": "", "message_read_coverage": {}, "run_message_summary_seq": 0, "message_inference_coverage": {}}'::jsonb"#
				                .to_owned())), ColumnDefinition::new("pending", FieldType::Jsonb)
				                .with_not_null(true)
				                .with_default(Some(r#"'{"data": {}, "recovery": {"retry": null, "lease_recovered": false}, "state_version": 1}'::jsonb"#
				                .to_owned())), ColumnDefinition::new("step", FieldType::Integer)
				                .with_not_null(true).with_default(Some("0".to_owned())),
				                ColumnDefinition::new("revision", FieldType::BigInteger)
				                .with_not_null(true).with_default(Some("0".to_owned())),
				                ColumnDefinition::new("error", FieldType::Text),
				                ColumnDefinition::new("lease_owner", FieldType::Uuid),
				                ColumnDefinition::new("lease_until", FieldType::TimestampTz),
				                ColumnDefinition::new("updated_at", FieldType::TimestampTz)
				                .with_not_null(true).with_default(Some(r#"now()"#.to_owned())),
				                ColumnDefinition::new("observed_input_seq", FieldType::BigInteger)
				                .with_not_null(true).with_default(Some("0".to_owned())),
				                ColumnDefinition::new("ledger_worker_ready", FieldType::Boolean)
				                .with_not_null(true).with_default(Some("false".to_owned())),
				                ColumnDefinition::new("pending_human_request_id", FieldType::Uuid)
				                .with_generated(Some(GeneratedColumnDefinition::raw_sql(r#"
						CASE
						WHEN ((((pending -> 'data'::text) ->> 'reason'::text) = ANY (ARRAY['human'::text, 'external_approval'::text, 'reconciliation'::text])) AND (jsonb_typeof(((pending -> 'data'::text) -> 'request_id'::text)) = 'string'::text) AND (((pending -> 'data'::text) ->> 'request_id'::text) ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'::text)) THEN (((pending -> 'data'::text) ->> 'request_id'::text))::uuid
						ELSE NULL::uuid
						END"#,
				                GeneratedStorage::Stored)))
				            ],
				            constraints: vec![
				                Constraint::Check { name : "runs_control_check".to_owned(), expression :
				                r#"(control = ANY (ARRAY['ACTIVE'::text, 'PAUSED'::text, 'CANCELLED'::text]))"#
				                .to_owned() }, Constraint::Check { name : "runs_counters".to_owned(),
				                expression :
				                r#"((step >= 0) AND (revision >= 0) AND (revision < '9223372036854775807'::bigint))"#
				                .to_owned() }, Constraint::Check { name : "runs_lease".to_owned(),
				                expression :
				                r#"COALESCE((((lease_owner IS NULL) = (lease_until IS NULL)) AND ((lease_until IS NULL) OR isfinite(lease_until))), false)"#
				                .to_owned() }, Constraint::Check { name : "runs_phase_check".to_owned(),
				                expression :
				                r#"(phase = ANY (ARRAY['READY'::text, 'THINKING'::text, 'TOOL_CALL'::text, 'WAITING'::text, 'COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text]))"#
				                .to_owned() }
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
