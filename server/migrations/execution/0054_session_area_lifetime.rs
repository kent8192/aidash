// reinhardt-migration-source: 1
// Source identity remains, with insertion-time same-Tenant KeyShare existence.
// Event.node is scalar emitter metadata. Workspace ownership may outlive the
// emitter's finite catalog audit, so retain its permanent same-Tenant identity.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0054_session_area_lifetime", "execution")
		.add_dependency(
			"execution",
			"0053_node_cancellation_jobs_row_level_security",
		)
		.database_only(true)
		.atomic(true)
		// Keep the canonical direct Tenant anchor beside the deletion's exact
		// composite ownership constraint; both survive reverse/reapplication.
		.add_operation(Operation::AddConstraintDefinition {
			table: "node_cancellation_jobs".into(),
			constraint: Constraint::ForeignKey {
				name: "node_cancellation_jobs_tenant_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_runs".into(),
			constraint: Constraint::ForeignKey {
				name: "core_runs_area_id_fkey".into(),
				columns: vec!["tenant".into(), "area_id".into()],
				referenced_table: "core_areas".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "events".into(),
			constraint: Constraint::ForeignKey {
				name: "events_node_fkey".into(),
				columns: vec!["tenant".into(), "node".into()],
				referenced_table: "nodes".into(),
				referenced_columns: vec!["tenant".into(), "name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "events".into(),
			constraint: Constraint::ForeignKey {
				name: "events_node_fkey".into(),
				columns: vec!["node".into(), "tenant".into()],
				referenced_table: "node_names".into(),
				referenced_columns: vec!["name".into(), "tenant".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
}
