// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0045_remote_connection_calls", "identity")
		.add_dependency("identity", "0044_oauth_clients_state")
        .add_dependency("federation", "0003_keys_indexes")
        .atomic(true)
        .database_only(true)
		.add_operation(Operation::CreateTable {
			name: "remote_connection_calls".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("grant_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("admission_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("idempotency_key", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("tool_key", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("resource", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("arguments_hash", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("connection_id", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("secret_id", FieldType::Uuid).with_not_null(false),
				ColumnDefinition::new("secret_version", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("secret_host", FieldType::Text).with_not_null(false),
				ColumnDefinition::new("status", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("result", FieldType::Jsonb).with_not_null(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
                Constraint::ForeignKey {
                    name: "remote_connection_calls_grant_fkey".into(),
                    columns: vec!["tenant".into(), "grant_id".into()],
                    referenced_table: "authorization_remote_grants".into(),
                    referenced_columns: vec!["tenant".into(), "id".into()],
                    on_delete: ForeignKeyAction::Cascade,
                    on_update: ForeignKeyAction::NoAction,
                    deferrable: None,
                },
                Constraint::Check {
                    name: "remote_connection_calls_status".into(),
                    expression: "status IN ('started','completed','failed') AND ((status = 'started' AND result IS NULL) OR (status <> 'started' AND result IS NOT NULL))".into(),
                },
            ],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
        })
        .add_operation(Operation::CreateNamedIndex {
            table: "remote_connection_calls".into(),
            name: "remote_connection_calls_grant_key".into(),
            columns: vec!["grant_id".into(), "idempotency_key".into()],
            unique: true,
            index_type: Some(IndexType::BTree),
            where_clause: None,
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        // RLS policy DDL is unsupported by Reinhardt migrations (#6770).
        .add_operation(Operation::RunSQL {
            sql: include_str!("sql/forward/0045_remote_connection_calls.sql").into(),
            reverse_sql: Some(include_str!("sql/backward/0045_remote_connection_calls.sql").into()),
        })
}
