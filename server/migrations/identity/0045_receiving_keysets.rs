// reinhardt-migration-source: 1
//! Receiver-local trust changes independently of authorization (ADR 0084).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0045_receiving_keysets", "identity")
		.add_dependency("identity", "0044_node_administration_target_state")
		.database_only(true)
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
		.add_operation(Operation::AddConstraintDefinition {
			table: "authorization_peer_graph_grants".into(),
			constraint: Constraint::Check {
				name: "authorization_peer_graph_grants_receiving_keys".into(),
				expression: "keys_revision > 0".into(),
			},
		})
}
