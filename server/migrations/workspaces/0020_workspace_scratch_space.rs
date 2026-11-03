// reinhardt-migration-source: 1
// Workspace bindings are backfilled before enforcing their required fields.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0020_workspace_scratch_space", "workspaces")
		.add_dependency("workspaces", "0019_scratch_node_index")
		.add_dependency("identity", "0007_model_state")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "workspaces".into(),
			column: ColumnDefinition::new("scratch_space_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "workspaces".into(),
			column: ColumnDefinition::new("created_by", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0020_workspace_scratch_space.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0020_workspace_scratch_space.sql").to_owned(),
			),
		})
		.add_operation(Operation::AlterColumn {
			table: "workspaces".into(),
			column: "scratch_space_id".into(),
			old_definition: Some(ColumnDefinition::new("scratch_space_id", FieldType::Uuid)),
			new_definition: ColumnDefinition::new("scratch_space_id", FieldType::Uuid)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "workspaces".into(),
			column: "created_by".into(),
			old_definition: Some(ColumnDefinition::new("created_by", FieldType::Text)),
			new_definition: ColumnDefinition::new("created_by", FieldType::Text)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "workspaces".into(),
			constraint: Constraint::ForeignKey {
				name: "workspaces_scratch_space_fkey".into(),
				columns: vec!["tenant".into(), "scratch_space_id".into()],
				referenced_table: "scratch_spaces".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
}
