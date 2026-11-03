// reinhardt-migration-source: 1
// Logical ORM relationship state; the physical foreign key was created by 0020.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0021_scratch_workspace_model_state", "workspaces")
        .add_operation(Operation::DropConstraintDefinition {
            table: "workspaces".to_string(),
            constraint: Constraint::ForeignKey {
                name: "workspaces_scratch_space_fkey".to_string(),
                columns: vec!["tenant".to_string(), "scratch_space_id".to_string()],
                referenced_table: "scratch_spaces".to_string(),
                referenced_columns: vec!["tenant".to_string(), "id".to_string()],
                on_delete: ForeignKeyAction::NoAction,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::AddConstraint {
            table: "workspaces".to_string(),
            constraint_sql: "CONSTRAINT workspaces_scratch_space_fkey FOREIGN KEY (tenant, scratch_space_id) REFERENCES scratch_spaces(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
                .to_string(),
        })
        .add_dependency("workspaces", "0020_workspace_scratch_space")
        .atomic(true)
        .with_initial(None)
        .state_only(true)
        .database_only(false)
}
