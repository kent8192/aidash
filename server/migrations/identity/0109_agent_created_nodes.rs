// reinhardt-migration-source: 1
// Agents create and delete Nodes as the person at the root of their chain
// (#1012, ADR 0143). A Node records the chain that created it: its root
// person, the creating Run and Task, and the tool call's key, so a retried
// call finds the Node its first call created. The columns are database-only,
// like `tasks.duty_id`: the ORM model never sets them and the writers use
// Reinhardt Query. A Node deletion may now be recorded for an Agent's chain.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0109_agent_created_nodes", "identity")
		.add_dependency("identity", "0101_budgets_row_level_security")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "nodes".into(),
			column: ColumnDefinition::new("created_by_subject", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "nodes".into(),
			column: ColumnDefinition::new("created_by_run_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "nodes".into(),
			column: ColumnDefinition::new("created_by_task_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "nodes".into(),
			column: ColumnDefinition::new("creation_key", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "nodes".into(), constraint: Constraint::Check { name: "nodes_creator".into(), expression: "created_by_kind IN ('identity','deployment_administrator','host_access') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND length(label) BETWEEN 1 AND 64".into() },
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "nodes".into(), constraint: Constraint::Check { name: "nodes_creator".into(), expression: "created_by_kind IN ('identity','deployment_administrator','host_access','agent') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND (created_by_kind = 'agent') = (created_by_subject IS NOT NULL) AND (created_by_kind <> 'agent' OR (created_by_identity IS NULL AND created_by_run_id IS NOT NULL AND created_by_task_id IS NOT NULL AND creation_key IS NOT NULL AND length(created_by_subject) BETWEEN 1 AND 256 AND length(creation_key) BETWEEN 1 AND 512)) AND length(label) BETWEEN 1 AND 64".into() },
		})
		// A retried tool call finds the Node its first call created.
		.add_operation(Operation::CreateNamedIndex {
			table: "nodes".into(),
			name: "nodes_creation_key".into(),
			columns: vec!["tenant".into(), "created_by_run_id".into(), "creation_key".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("creation_key IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// A flow's stop lists the Nodes its Tasks created.
		.add_operation(Operation::CreateNamedIndex {
			table: "nodes".into(),
			name: "nodes_created_by_task".into(),
			columns: vec!["tenant".into(), "created_by_task_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("created_by_task_id IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "node_deletions".into(), constraint: Constraint::Check { name: "node_deletions_state".into(), expression: "deleted_by_kind IN ('tenant_administrator','deployment_administrator') AND (deleted_by_kind <> 'tenant_administrator' OR deleted_by_identity IS NOT NULL) AND purge_after >= deleted_at + INTERVAL '24 hours' AND purge_after <= deleted_at + INTERVAL '2160 hours' AND (restored_at IS NULL) = (restored_by_kind IS NULL) AND (restored_by_kind IS NULL OR restored_by_kind IN ('tenant_administrator','deployment_administrator')) AND (restored_by_kind IS DISTINCT FROM 'tenant_administrator' OR restored_by_identity IS NOT NULL) AND (restored_at IS NULL OR restored_at >= deleted_at) AND (restored_at IS NULL OR purged_at IS NULL) AND (purge_step IS NULL OR purge_step IN ('capability_objects','rows','finish')) AND purge_failures >= 0 AND (purged_at IS NULL OR purge_step IS NULL) AND (purge_failures = 0 OR purge_error IS NOT NULL) AND (audit_erased_at IS NULL OR purged_at IS NOT NULL) AND (purge_lease IS NULL) = (purge_leased_until IS NULL)".into() },
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "node_deletions".into(), constraint: Constraint::Check { name: "node_deletions_state".into(), expression: "deleted_by_kind IN ('tenant_administrator','deployment_administrator','agent') AND (deleted_by_kind <> 'tenant_administrator' OR deleted_by_identity IS NOT NULL) AND purge_after >= deleted_at + INTERVAL '24 hours' AND purge_after <= deleted_at + INTERVAL '2160 hours' AND (restored_at IS NULL) = (restored_by_kind IS NULL) AND (restored_by_kind IS NULL OR restored_by_kind IN ('tenant_administrator','deployment_administrator')) AND (restored_by_kind IS DISTINCT FROM 'tenant_administrator' OR restored_by_identity IS NOT NULL) AND (restored_at IS NULL OR restored_at >= deleted_at) AND (restored_at IS NULL OR purged_at IS NULL) AND (purge_step IS NULL OR purge_step IN ('capability_objects','rows','finish')) AND purge_failures >= 0 AND (purged_at IS NULL OR purge_step IS NULL) AND (purge_failures = 0 OR purge_error IS NOT NULL) AND (audit_erased_at IS NULL OR purged_at IS NOT NULL) AND (purge_lease IS NULL) = (purge_leased_until IS NULL)".into() },
		})
}
