// reinhardt-migration-source: 1
// Frozen PostgreSQL baseline from develop/0.1.0 d120162 (54 legacy migrations).
// Schema operations use Reinhardt; unsupported migration operations live in sql/.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0004_references", "knowledge")
		.database_only(true)
		.add_dependency("identity", "0004_references")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_agent_memory".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_agent_memory_entry_id_fkey".to_owned(),
				columns: vec!["entry_id".to_owned()],
				referenced_table: "semantic_entries".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_collections".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_collections_workspace_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
				referenced_table: "semantic_indexes".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_entries".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_entries_workspace_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
				referenced_table: "semantic_indexes".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_indexes".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_indexes_workspace_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "workspace_id".to_owned()],
				referenced_table: "workspaces".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_points".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_points_collection_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "collection".to_owned()],
				referenced_table: "semantic_collections".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "collection".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_points".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_points_entry_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "entry_id".to_owned()],
				referenced_table: "semantic_entries".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_run_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_run_reads_entry_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "entry_id".to_owned()],
				referenced_table: "semantic_entries".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_run_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_run_reads_run_id_fkey".to_owned(),
				columns: vec!["tenant".to_owned(), "run_id".to_owned()],
				referenced_table: "runs".to_owned(),
				referenced_columns: vec!["tenant".to_owned(), "id".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_collections".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_collections_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_entries".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_entries_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_history".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_history_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_indexes".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_indexes_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_points".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_points_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_remote_attempts".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_remote_attempts_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_remote_operations".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_remote_operations_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_remote_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_remote_reads_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_remote_receipts".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_remote_receipts_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_run_reads".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "semantic_run_reads_tenant_root_fkey".to_owned(),
				columns: vec!["tenant".to_owned()],
				referenced_table: "tenants".to_owned(),
				referenced_columns: vec!["name".to_owned()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
