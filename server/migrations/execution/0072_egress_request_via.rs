// reinhardt-migration-source: 1
// Which path admitted an Egress request: an operation, a Live preview Pod's
// session, or the Preview gateway on a page's behalf (ADR 0069, ADR 0070).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0072_egress_request_via", "execution")
		.add_dependency("execution", "0071_egress_session_requests_state")
		.database_only(true)
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("via", FieldType::Text)
				.with_not_null(true)
				.with_default(Some("operation".into())),
			mysql_options: None,
		})
}
