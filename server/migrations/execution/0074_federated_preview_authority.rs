// reinhardt-migration-source: 1
// Gateway sessions carry the exact admitted peer identity for each remote
// ticket, so every request can recheck the current Peer Mapping.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0074_federated_preview_authority", "execution")
		.add_dependency("execution", "0073_egress_request_via_state")
		.add_dependency("federation", "0013_registered_preview_origin")
		.add_operation(Operation::AddColumn {
			table: "core_preview_tickets".into(),
			column: ColumnDefinition::new("peer_sender", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "core_preview_tickets".into(),
			column: ColumnDefinition::new("peer_receiver", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "core_preview_tickets".into(),
			column: ColumnDefinition::new("peer_subject", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "core_preview_tickets".into(),
			column: ColumnDefinition::new("peer_run_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "core_preview_sessions".into(),
			column: ColumnDefinition::new("peer_sender", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "core_preview_sessions".into(),
			column: ColumnDefinition::new("peer_receiver", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "core_preview_sessions".into(),
			column: ColumnDefinition::new("peer_subject", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "core_preview_sessions".into(),
			column: ColumnDefinition::new("peer_run_id", FieldType::Uuid),
			mysql_options: None,
		})
		.atomic(true)
}
