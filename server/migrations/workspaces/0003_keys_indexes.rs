// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0003_keys_indexes", "workspaces")
		.database_only(true)
		.add_dependency("registry", "0003_keys_indexes")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "artifacts".to_owned(),
			constraint: Constraint::Unique {
				name: "artifacts_idempotency_key_key".to_owned(),
				columns: vec!["idempotency_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "artifacts".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "artifacts_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".to_owned(),
			constraint: Constraint::Unique {
				name: "channel_attachment_idempotency".to_owned(),
				columns: vec![
					"workspace_id".to_owned(),
					"uploaded_by".to_owned(),
					"idempotency_key".to_owned(),
				],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".to_owned(),
			constraint: Constraint::Unique {
				name: "channel_attachment_scope_key".to_owned(),
				columns: vec!["workspace_id".to_owned(), "id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_attachments".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "channel_attachments_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_message_context".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "channel_message_context_pkey".to_owned(),
				columns: vec!["message_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_threads".to_owned(),
			constraint: Constraint::Unique {
				name: "channel_thread_scope_key".to_owned(),
				columns: vec!["workspace_id".to_owned(), "id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_threads".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "channel_threads_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "channel_threads".to_owned(),
			constraint: Constraint::Unique {
				name: "channel_threads_root_message_id_key".to_owned(),
				columns: vec!["root_message_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "conversations".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "conversations_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "messages".to_owned(),
			constraint: Constraint::Unique {
				name: "messages_idempotency_key_key".to_owned(),
				columns: vec!["idempotency_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "messages".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "messages_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "task_dependencies".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "task_dependencies_pkey".to_owned(),
				columns: vec!["task_id".to_owned(), "dependency_id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tasks".to_owned(),
			constraint: Constraint::Unique {
				name: "tasks_completion_key_key".to_owned(),
				columns: vec!["completion_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tasks".to_owned(),
			constraint: Constraint::Unique {
				name: "tasks_creation_key_key".to_owned(),
				columns: vec!["creation_key".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tasks".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "tasks_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "workspaces".to_owned(),
			constraint: Constraint::PrimaryKey {
				name: "workspaces_pkey".to_owned(),
				columns: vec!["id".to_owned()],
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "channel_attachments".to_owned(),
			name: "channel_attachment_message_lookup".to_owned(),
			columns: vec!["workspace_id".to_owned(), "message_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "messages".to_owned(),
			name: "channel_message_history_order".to_owned(),
			columns: vec![
				"workspace_id".to_owned(),
				"created_at".to_owned(),
				"id".to_owned(),
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
			table: "messages".to_owned(),
			name: "channel_message_scope_key".to_owned(),
			columns: vec!["workspace_id".to_owned(), "id".to_owned()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "channel_message_context".to_owned(),
			name: "channel_reply_lookup".to_owned(),
			columns: vec![
				"workspace_id".to_owned(),
				"thread_id".to_owned(),
				"message_id".to_owned(),
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
			table: "task_dependencies".to_owned(),
			name: "task_dependencies_target".to_owned(),
			columns: vec!["dependency_id".to_owned(), "workspace_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "tasks".to_owned(),
			name: "tasks_id_workspace_unique".to_owned(),
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
			table: "tasks".to_owned(),
			name: "tasks_workspace".to_owned(),
			columns: vec!["workspace_id".to_owned(), "created_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
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
