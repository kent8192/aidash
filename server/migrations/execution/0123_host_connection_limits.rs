// reinhardt-migration-source: 1
// Host connection limits (#845): why a connection that held its limits did not
// complete and how long it waited; per-minute counts of connections that ended
// before (tables in 0124 and 0125); and no suspension state, because nothing
// suspends a scope any longer.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0123_host_connection_limits", "execution")
		.add_dependency("execution", "0122_invocation_refused_state")
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("error", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("wait_ms", FieldType::BigInteger),
			mysql_options: None,
		})
		.add_operation(Operation::DropColumn {
			table: "egress_scope_state".into(),
			column: "suspended".into(),
			old_definition: Some(
				ColumnDefinition::new("suspended", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".to_owned())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "egress_scope_state".into(),
			column: "window_started_at".into(),
			old_definition: Some(
				ColumnDefinition::new("window_started_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".to_owned())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "egress_scope_state".into(),
			column: "connections_in_window".into(),
			old_definition: Some(
				ColumnDefinition::new("connections_in_window", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("0".to_owned())),
			),
		})
		.atomic(true)
}
