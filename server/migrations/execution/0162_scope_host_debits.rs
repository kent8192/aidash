// reinhardt-migration-source: 1
//! Host reservation receipts survive a lost commit answer (#1037).
//! Reinhardt cannot express the forced Tenant row policy, so use SQL assets.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0162_scope_host_debits", "execution")
		.add_dependency("execution", "0161_generation_subnode_cleanup")
		.database_only(true)
		.atomic(true)
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
			constraints: vec![
				Constraint::ForeignKey {
					name: "egress_scope_host_debits_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "egress_scope_host_debits_scope_fkey".into(),
					columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
					referenced_table: "egress_scope_state".into(),
					referenced_columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "egress_scope_host_debits_nonnegative".into(),
					expression: "bytes >= 0".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "egress_scope_host_debits".into(),
			columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0162_scope_host_debits.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0162_scope_host_debits.sql").into()),
		})
}
