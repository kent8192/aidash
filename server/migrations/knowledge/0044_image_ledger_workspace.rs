// reinhardt-migration-source: 1
// The physical Workspace FK and JSONB checks are separate from scalar model state.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0044_image_ledger_workspace", "knowledge")
		.add_dependency("knowledge", "0043_image_ledgers")
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_image_ledgers".into(),
			constraint: Constraint::ForeignKey {
				name: "semantic_image_ledgers_workspace_fkey".into(),
				columns: vec!["tenant".into(), "workspace_id".into()],
				referenced_table: "workspaces".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "semantic_image_ledgers".into(),
			constraint: Constraint::Check {
				name: "semantic_image_ledgers_shape".into(),
				expression:
					"jsonb_typeof(slots) = 'object' AND jsonb_typeof(authorities) = 'object'".into(),
			},
		})
		.atomic(true)
		.database_only(true)
}
