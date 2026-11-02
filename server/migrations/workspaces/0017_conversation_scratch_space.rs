// reinhardt-migration-source: 1
// A conversation keeps one Scratch Space for its lifetime, in the same Tenant.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0017_conversation_scratch_space", "workspaces")
		.add_dependency("workspaces", "0016_scratch_spaces_row_level_security")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "conversations".into(),
			column: ColumnDefinition::new("scratch_space_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0017_conversation_scratch_space.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0017_conversation_scratch_space.sql").to_owned(),
			),
		})
		.add_operation(Operation::AlterColumn {
			table: "conversations".into(),
			column: "scratch_space_id".into(),
			old_definition: Some(ColumnDefinition::new("scratch_space_id", FieldType::Uuid)),
			new_definition: ColumnDefinition::new("scratch_space_id", FieldType::Uuid)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "conversations".into(),
			constraint: Constraint::ForeignKey {
				name: "conversations_scratch_space_fkey".into(),
				columns: vec!["tenant".into(), "scratch_space_id".into()],
				referenced_table: "scratch_spaces".into(),
				referenced_columns: vec!["tenant".into(), "id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::Restrict,
				deferrable: None,
			},
		})
}
