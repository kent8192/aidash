// reinhardt-migration-source: 1
//! Model state for Host reservation receipts.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0159_scope_host_debit_state", "execution")
		.add_dependency("execution", "0158_scope_host_debits")
		.state_only(true)
		.add_operation(Operation::CreateTable {
			name: "egress_scope_host_debits".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("day", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("sent", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("bytes", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
