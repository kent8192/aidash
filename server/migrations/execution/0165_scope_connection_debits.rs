// reinhardt-migration-source: 1
//! Store the cumulative Host reservation on the connection instead of receipts.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0165_scope_connection_debits", "execution")
		.add_dependency("execution", "0163_scope_host_debit_state")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("charged_sent", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("charged_received", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("scope_charged_sent", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("scope_charged_received", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::CreateTable {
			name: "egress_usage_daily".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("day", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("sent_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("received_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("cost_nanocredits", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![Constraint::Unique {
				name: "egress_usage_daily_tenant_day_key".into(),
				columns: vec!["tenant".into(), "day".into()],
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "egress_scope_connections".into(),
			columns: vec!["tenant".into(), "run_id".into(), "host".into(), "created_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(outcome = 'completed' AND run_id IS NOT NULL)".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// Reinhardt cannot express forced Tenant row-level policies.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0165_scope_connection_debits.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0165_scope_connection_debits.sql").into()),
		})
		// On reversal, DropTable recreates the columns from model state. This
		// asset restores the database-only properties, including forced RLS.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/backward/0162_scope_host_debits.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0165_restore_scope_host_debits.sql").into()),
		})
		.add_operation(Operation::DropTable {
			name: "egress_scope_host_debits".into(),
		})
}
