// reinhardt-migration-source: 1
//! The `jti` of every Federation token a receiving Node verified, kept until
//! the token's `exp` plus 30 seconds so a replay is refused (ADR 0064, #989).
//! Reinhardt migrations do not express row-level policies, so the policy is a
//! SQL asset that `server/src/database/policy.rs` renders and a test compares.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0024_federation_token_ids", "federation")
		.add_dependency("federation", "0023_delegation_unused_columns_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "federation_token_ids".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("deployment", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("jti", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::PrimaryKey {
					name: "federation_token_ids_pkey".into(),
					columns: vec![
						"tenant".into(),
						"node".into(),
						"deployment".into(),
						"jti".into(),
					],
				},
				Constraint::ForeignKey {
					name: "federation_token_ids_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		// The insert path deletes a Node's expired rows by this index.
		.add_operation(Operation::CreateNamedIndex {
			table: "federation_token_ids".into(),
			name: "federation_token_ids_expiry".into(),
			columns: vec!["tenant".into(), "node".into(), "expires_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0024_federation_token_ids.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0024_federation_token_ids.sql").into()),
		})
}
