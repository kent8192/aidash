// reinhardt-migration-source: 1
//! Home-owned human continuations do not create a shadow execution Run.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0010_home_human_requests", "identity")
		.database_only(true)
		.add_dependency("identity", "0009_desktop_sessions")
		.add_dependency("federation", "0007_model_state")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").into()),
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_remote_execution".into(),
			column: ColumnDefinition::new("human_requests", FieldType::Jsonb)
				.with_not_null(true)
				.with_default(Some("'[]'::jsonb".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "delegations".into(),
			column: ColumnDefinition::new("human_requests", FieldType::Jsonb)
				.with_not_null(true)
				.with_default(Some("'[]'::jsonb".into())),
			mysql_options: None,
		})
}
