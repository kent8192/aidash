// reinhardt-migration-source: 1
// Session-lifetime Egress requests have no core operation, only a Live preview.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0070_egress_session_requests", "execution")
		.add_dependency("execution", "0069_live_previews_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0070_egress_session_requests.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0070_egress_session_requests.sql").to_owned(),
			),
		})
}
