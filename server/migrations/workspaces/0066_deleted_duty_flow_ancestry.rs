// reinhardt-migration-source: 1
// A Duty's surviving Tasks retain its creating Task for flow traversal after
// deletion (#1011, #1012). The separate link also works across Threads. It is
// database-only because the Duty repository writes it with Reinhardt Query.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0066_deleted_duty_flow_ancestry", "workspaces")
		.add_dependency("workspaces", "0065_duty_pending_events")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "tasks".into(),
			column: ColumnDefinition::new("deleted_duty_parent_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "tasks".into(),
			name: "tasks_deleted_duty_parent".into(),
			columns: vec!["tenant".into(), "deleted_duty_parent_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("deleted_duty_parent_id IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
