// reinhardt-migration-source: 1
// A Duty an Agent sets up records the Task whose Run set it up (#1012), so a
// flow's stop pauses every Duty its Runs set up, and the org chart names the
// Task that created it. It is database-only, like `tasks.duty_id`: the ORM
// model never sets it and the writers use Reinhardt Query. A successor taking
// a Duty over clears it with the creating Run.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0047_duty_creating_task", "workspaces")
		.add_dependency("workspaces", "0041_duty_scripts_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "duties".into(),
			column: ColumnDefinition::new("created_by_task_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duties".into(),
			name: "duties_created_by_task".into(),
			columns: vec!["tenant".into(), "created_by_task_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("created_by_task_id IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
