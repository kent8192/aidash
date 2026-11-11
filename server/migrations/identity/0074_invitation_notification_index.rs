// reinhardt-migration-source: 1
// Keep the recent Invitation notification feed bounded as audit history grows.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0074_invitation_notification_index", "identity")
		.add_dependency("identity", "0073_invitation_direct_join")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateNamedIndex {
			table: "dashboard_administration_history".into(),
			name: "dashboard_history_tenant_action_time".into(),
			columns: vec![
				"tenant".into(),
				"action".into(),
				"occurred_at".into(),
				"id".into(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
