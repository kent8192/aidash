// reinhardt-migration-source: 1
//! Receiver-local trust changes independently of authorization (ADR 0084).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0015_receiving_keysets", "federation")
		.add_dependency("federation", "0014_deployment_key_rollovers_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "node_registrations".into(),
			column: ColumnDefinition::new("source_keys", FieldType::Text).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "node_registrations".into(),
			column: ColumnDefinition::new("keys_revision", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("1".to_owned())),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "node_registrations".into(),
			constraint: Constraint::Check {
				name: "node_registrations_receiving_keys".into(),
				expression: "keys_revision > 0 AND (source_keys IS NULL) = (source_key_id IS NULL)"
					.into(),
			},
		})
		.add_operation(Operation::AddColumn {
			table: "node_registration_history".into(),
			column: ColumnDefinition::new("source_keys", FieldType::Text).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "node_registration_history".into(),
			column: ColumnDefinition::new("keys_revision", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("1".to_owned())),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "node_registration_history".into(),
			constraint: Constraint::Check {
				name: "node_registration_history_receiving_keys".into(),
				expression: "keys_revision > 0 AND (source_keys IS NULL) = (source_key_id IS NULL)"
					.into(),
			},
		})
}
