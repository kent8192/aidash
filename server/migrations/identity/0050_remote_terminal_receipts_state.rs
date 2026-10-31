// reinhardt-migration-source: 1
//! Scalar recovery identities; permanent physical ownership stays database-only.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0050_remote_terminal_receipts_state", "identity")
		.add_dependency("identity", "0049_remote_terminal_receipts")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "authorization_remote_terminal_receipts".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source_node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("activation", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
