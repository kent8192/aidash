// reinhardt-migration-source: 1
//! Receiver-local trust changes independently of authorization (ADR 0084).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0046_receiving_keysets_state", "identity")
		.add_dependency("identity", "0045_receiving_keysets")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "authorization_peer_graph_grants".into(),
			column: ColumnDefinition::new("source_keys", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_peer_graph_grants".into(),
			column: ColumnDefinition::new("keys_revision", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("1".to_owned())),
			mysql_options: None,
		})
}
