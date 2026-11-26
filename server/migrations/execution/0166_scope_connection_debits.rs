// reinhardt-migration-source: 1
//! Store the cumulative Host reservation on the connection instead of receipts.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0166_scope_connection_debits", "execution")
		.add_dependency("execution", "0165_background_time_limit")
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
			columns: vec!["run_id".into()],
			unique: false,
			index_type: None,
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		// The Tenant root reference every Tenant-owned table carries. Like
		// 0077/0079, the logical state keeps no foreign key, so the physical
		// key is added in SQL, apart from the rendered policy template.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0166_egress_usage_daily_tenant_key.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0166_egress_usage_daily_tenant_key.sql").into()),
		})
		// Reinhardt cannot express forced Tenant row-level policies.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0166_scope_connection_debits.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0166_scope_connection_debits.sql").into()),
		})
		// On reversal, DropTable recreates the columns from model state. This
		// asset restores the database-only properties, including forced RLS.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/backward/0162_scope_host_debits.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0166_restore_scope_host_debits.sql").into()),
		})
		.add_operation(Operation::DropTable {
			name: "egress_scope_host_debits".into(),
		})
}
