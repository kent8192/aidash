// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0003_keys_indexes", "execution")
		.database_only(true)
		.add_dependency("workspaces", "0002_tables")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "activation_quarantine".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "activation_quarantine_pkey".to_owned(),
				columns: vec!["digest".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_areas".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "core_areas_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_objects".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "core_objects_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_operations".to_owned(),
			constraint: Constraint::Unique {
				name: "core_operation_key".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"principal".to_owned(),
					"request_key".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_operations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "core_operations_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_quotas".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "core_quotas_pkey".to_owned(),
				columns: vec!["tenant".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_records".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "core_records_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_requests".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "core_requests_pkey".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"principal".to_owned(),
					"key".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_runs".to_owned(),
			constraint: Constraint::Unique {
				name: "core_run_order".to_owned(),
				columns: vec!["area_id".to_owned(), "sequence".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_runs".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "core_runs_pkey".to_owned(),
				columns: vec!["run_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_areas".to_owned(),
			constraint: Constraint::Unique {
				name: "core_session_identity".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"home_node".to_owned(),
					"workspace_id".to_owned(),
					"thread_id".to_owned(),
					"agent_id".to_owned(),
					"owner".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_task_sessions".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "core_task_sessions_pkey".to_owned(),
				columns: vec!["task_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "events".to_owned(),
			constraint: Constraint::Unique {
				name: "events_id_key".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "events".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "events_pkey".to_owned(),
				columns: vec!["sequence".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_budgets".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_budgets_pkey".to_owned(),
				columns: vec!["request_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_compaction_usage".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_compaction_usage_pkey".to_owned(),
				columns: vec!["request_id".to_owned(), "attempt_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_embedding_usage_pkey".to_owned(),
				columns: vec!["request_id".to_owned(), "attempt_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_history".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_history_pkey".to_owned(),
				columns: vec!["sequence".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_policies".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_policies_pkey".to_owned(),
				columns: vec!["tenant".to_owned(), "id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_policy_history".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_policy_history_pkey".to_owned(),
				columns: vec![
					"tenant".to_owned(),
					"policy_id".to_owned(),
					"revision".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_dispatches".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_remote_dispatches_pkey".to_owned(),
				columns: vec!["attempt_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_finalizations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_remote_finalizations_pkey".to_owned(),
				columns: vec!["attempt_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_intents".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_remote_intents_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_remote_usage".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_remote_usage_pkey".to_owned(),
				columns: vec!["request_id".to_owned(), "attempt_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::Unique {
				name: "generation_requests_agent_id_key".to_owned(),
				columns: vec!["agent_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_requests".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_requests_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_usage".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "generation_usage_pkey".to_owned(),
				columns: vec!["request_id".to_owned(), "attempt_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "human_requests".to_owned(),
			constraint: Constraint::Unique {
				name: "human_requests_id_run_id_key".to_owned(),
				columns: vec!["id".to_owned(), "run_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "human_requests".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "human_requests_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "human_requests".to_owned(),
			constraint: Constraint::Unique {
				name: "human_requests_request_key_key".to_owned(),
				columns: vec!["request_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inbox".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "inbox_pkey".to_owned(),
				columns: vec!["event_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "invocations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "invocations_pkey".to_owned(),
				columns: vec!["idempotency_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "memory".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "memory_next_pkey".to_owned(),
				columns: vec![
					"agent_id".to_owned(),
					"agent_version".to_owned(),
					"workspace_id".to_owned(),
					"home_node".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_activations".to_owned(),
			constraint: Constraint::Unique {
				name: "run_activations_id_key".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_activations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "run_activations_pkey".to_owned(),
				columns: vec!["generation".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_inputs".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "run_inputs_pkey".to_owned(),
				columns: vec!["seq".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "run_inputs".to_owned(),
			constraint: Constraint::Unique {
				name: "run_inputs_run_key".to_owned(),
				columns: vec!["run_id".to_owned(), "idempotency_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "runs".to_owned(),
			constraint: Constraint::Unique {
				name: "runs_home_node_task_id_key".to_owned(),
				columns: vec!["home_node".to_owned(), "task_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "runs".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "runs_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "runs".to_owned(),
			name: "activation_dependency_runs".to_owned(),
			columns: vec!["task_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(
				r#"(phase <> ALL (ARRAY['COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text]))"#
					.to_owned(),
			),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "run_activations".to_owned(),
			name: "activation_due".to_owned(),
			columns: vec!["due_at".to_owned(), "generation".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(state <> 'settled'::text)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "run_activations".to_owned(),
			name: "activation_run_generation".to_owned(),
			columns: vec!["run_id".to_owned(), "generation".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
				            table: "core_operations".to_owned(),
				            name: "core_operation_pending_queue".to_owned(),
				            columns: vec!["updated_at".to_owned()],
				            unique: false,
				            index_type: Some(IndexType::BTree),
				            where_clause: Some(
				                r#"(state = ANY (ARRAY['prepared'::text, 'submitted'::text, 'running'::text, 'cancelling'::text]))"#
				                    .to_owned(),
				            ),
				            concurrently: false,
				            expressions: None,
				            mysql_options: None,
				            operator_class: None,
				        })
		.add_operation(Operation::CreateNamedIndex {
			table: "runs".to_owned(),
			name: "dashboard_status_waiting_runs".to_owned(),
			columns: vec!["id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(
				r#"((control = 'PAUSED'::text) AND (error = 'identity status unavailable'::text))"#
					.to_owned(),
			),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
				            table: "events".to_owned(),
				            name: "events_marketplace_package_sequence".to_owned(),
				            columns: vec![],
				            unique: false,
				            index_type: Some(IndexType::BTree),
				            where_clause: Some(
				                r#"((workspace_id IS NULL) AND (kind ~~ 'marketplace.%'::text) AND (kind <> 'marketplace.audit'::text))"#
				                    .to_owned(),
				            ),
				            concurrently: false,
				            expressions: Some(
				                vec![r#"((data ->> 'key'::text))"#.to_owned(), "sequence".to_owned()],
				            ),
				            mysql_options: None,
				            operator_class: None,
				        })
		.add_operation(Operation::CreateNamedIndex {
				            table: "events".to_owned(),
				            name: "events_marketplace_tenant_sequence".to_owned(),
				            columns: vec![],
				            unique: false,
				            index_type: Some(IndexType::BTree),
				            where_clause: Some(
				                r#"((workspace_id IS NULL) AND (kind ~~ 'marketplace.%'::text) AND (kind <> 'marketplace.audit'::text))"#
				                    .to_owned(),
				            ),
				            concurrently: false,
				            expressions: Some(
				                vec![r#"((data ->> 'tenant'::text))"#.to_owned(), "sequence".to_owned()],
				            ),
				            mysql_options: None,
				            operator_class: None,
				        })
		.add_operation(Operation::CreateNamedIndex {
			table: "events".to_owned(),
			name: "events_outbox".to_owned(),
			columns: vec!["sequence".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(published_at IS NULL)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "events".to_owned(),
			name: "events_retry".to_owned(),
			columns: vec!["next_attempt_at".to_owned(), "sequence".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(published_at IS NULL)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "events".to_owned(),
			name: "events_workspace".to_owned(),
			columns: vec!["workspace_id".to_owned(), "sequence".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
				            table: "generation_remote_dispatches".to_owned(),
				            name: "generation_remote_dispatches_pending_finalization".to_owned(),
				            columns: vec!["created_at".to_owned()],
				            unique: false,
				            index_type: Some(IndexType::BTree),
				            where_clause: Some(
				                r#"((state = ANY (ARRAY['ABORTED'::text, 'SETTLED'::text])) AND (peer_finalized = false))"#
				                    .to_owned(),
				            ),
				            concurrently: false,
				            expressions: None,
				            mysql_options: None,
				            operator_class: None,
				        })
		.add_operation(Operation::CreateNamedIndex {
			table: "generation_remote_dispatches".to_owned(),
			name: "generation_remote_dispatches_preparing".to_owned(),
			columns: vec!["created_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(state = 'PREPARING'::text)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "generation_remote_intents".to_owned(),
			name: "generation_remote_intents_cancel_retry".to_owned(),
			columns: vec![
				"cancelled".to_owned(),
				"cancel_delivered".to_owned(),
				"cancel_retry_at".to_owned(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "generation_remote_usage".to_owned(),
			name: "generation_remote_usage_attempt_digest".to_owned(),
			columns: vec!["attempt_id".to_owned(), "digest".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
				            table: "generation_requests".to_owned(),
				            name: "generation_requests_home_task".to_owned(),
				            columns: vec!["home_node".to_owned(), "task_id".to_owned()],
				            unique: true,
				            index_type: Some(IndexType::BTree),
				            where_clause: Some(
				                r#"((home_node <> ''::text) AND (status = ANY (ARRAY['PENDING_APPROVAL'::text, 'QUEUED'::text, 'ACTIVE'::text])))"#
				                    .to_owned(),
				            ),
				            concurrently: false,
				            expressions: None,
				            mysql_options: None,
				            operator_class: None,
				        })
		.add_operation(Operation::CreateNamedIndex {
			table: "generation_requests".to_owned(),
			name: "generation_requests_local_task".to_owned(),
			columns: vec!["task_id".to_owned()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some(r#"(home_node = ''::text)"#.to_owned()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "generation_requests".to_owned(),
			name: "generation_requests_policy".to_owned(),
			columns: vec![
				"tenant".to_owned(),
				"policy_id".to_owned(),
				"status".to_owned(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "runs".to_owned(),
			name: "runs_id_workspace_unique".to_owned(),
			columns: vec!["id".to_owned(), "workspace_id".to_owned()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "runs".to_owned(),
			name: "runs_ready".to_owned(),
			columns: vec!["updated_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some(
				r#"(phase <> ALL (ARRAY['COMPLETED'::text, 'FAILED'::text, 'CANCELLED'::text]))"#
					.to_owned(),
			),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
