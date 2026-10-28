// reinhardt-migration-source: 1
//! A tool invocation records the person's OAuth Connection, never its token (ADR 0100).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0082_invocation_connections", "execution")
		.add_dependency("execution", "0081_egress_secret_records")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "invocations".into(),
			column: ColumnDefinition::new("connection_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
}
