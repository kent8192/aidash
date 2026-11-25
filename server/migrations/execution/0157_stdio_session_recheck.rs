// reinhardt-migration-source: 1
//! Remember the authority and Scratch Space of a potentially live stdio session.
use reinhardt::db::migrations::{FieldType, prelude::*};

pub(super) fn migration() -> Migration {
	Migration::new("0157_stdio_session_recheck", "execution")
		.add_dependency("execution", "0156_model_call_state")
		.database_only(true)
		.add_operation(Operation::AddColumn {
			table: "task_executions".into(),
			column: ColumnDefinition::new("stdio_session", FieldType::Jsonb),
			mysql_options: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "task_executions".into(),
			columns: vec!["id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("stdio_session IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.atomic(true)
}
