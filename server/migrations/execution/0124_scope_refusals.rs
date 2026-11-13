// reinhardt-migration-source: 1
// Per-minute counts of Host connections that ended before they held a limit
// (#845). Database-only, like 0109: the ORM state is in 0125, and Reinhardt
// migrations do not express the row policy.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0124_scope_refusals", "execution")
		.add_dependency("execution", "0123_host_connection_limits")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "egress_scope_refusals".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("minute", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("host", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("count", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "egress_scope_refusals_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "egress_scope_refusals_scope_fkey".into(),
					columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
					referenced_table: "egress_scope_state".into(),
					referenced_columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "egress_scope_refusal_valid".into(),
					expression:
						"char_length(host) <= 253 AND char_length(reason) <= 64 AND count > 0"
							.into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "egress_scope_refusals".into(),
			columns: vec![
				"tenant".into(),
				"node".into(),
				"scope_id".into(),
				"minute".into(),
			],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0124_scope_refusals.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0124_scope_refusals.sql").into()),
		})
}
