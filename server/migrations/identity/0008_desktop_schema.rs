// reinhardt-migration-source: 1
// Preserve the PostgreSQL schema from legacy migration 55 with typed operations.
// FieldType::Bytea keeps byte columns valid until generic Binary rendering is fixed
// (reinhardt-web#6637); then normal generated DDL can replace the state/DB split.
// Keep physical operations before the model snapshot until file-state replay
// excludes database-only operations (reinhardt-web#6638).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0008_desktop_schema", "identity")
		.database_only(true)
		.add_dependency("identity", "0007_model_state")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").to_owned()),
		})
		.add_operation(Operation::CreateTable {
			name: "desktop_handoffs".to_string(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(true)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("state", FieldType::Text)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("challenge", FieldType::Text)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("redirect_uri", FieldType::Text)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("origin", FieldType::Text)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("browser_session_id", FieldType::Uuid)
					.with_not_null(false)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("code_hash", FieldType::Bytea)
					.with_not_null(false)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
			],
			constraints: vec![Constraint::Unique {
				name: "desktop_handoffs_code_hash_uniq_4c740061".to_string(),
				columns: vec!["code_hash".to_string()],
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "desktop_refresh_credentials".to_string(),
			columns: vec![
				ColumnDefinition::new("token_hash", FieldType::Bytea)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(true)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("session_id", FieldType::Uuid)
					.with_not_null(true)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
				ColumnDefinition::new("next_hash", FieldType::Bytea)
					.with_not_null(false)
					.with_unique(false)
					.with_primary_key(false)
					.with_auto_increment(false)
					.with_default(None)
					.with_generated(None)
					.with_domain_option(None),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_sessions".to_string(),
			column: ColumnDefinition::new("desktop", FieldType::Boolean)
				.with_not_null(true)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(Some("false".to_string()))
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_sessions".to_string(),
			column: ColumnDefinition::new("desktop_idle_seconds", FieldType::BigInteger)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_sessions".to_string(),
			column: ColumnDefinition::new("access_expires_at", FieldType::TimestampTz)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "desktop_handoffs".to_owned(),
			constraint: Constraint::Unique {
				name: "desktop_handoffs_code_hash_uniq_4c740061".to_owned(),
				columns: vec!["code_hash".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "desktop_handoffs".to_owned(),
			constraint: Constraint::Unique {
				name: "desktop_handoffs_code_hash_key".to_owned(),
				columns: vec!["code_hash".to_owned()],
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "desktop_handoffs".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "desktop_handoffs_browser_session_id_fkey".to_owned(),
				columns: vec!["browser_session_id".to_owned()],
				referenced_table: "dashboard_sessions".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "desktop_refresh_credentials".to_owned(),
			constraint: Constraint::ForeignKey {
				name: "desktop_refresh_credentials_session_id_fkey".to_owned(),
				columns: vec!["session_id".to_owned()],
				referenced_table: "dashboard_sessions".to_owned(),
				referenced_columns: vec!["id".to_owned()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "desktop_handoffs".to_owned(),
			name: "desktop_handoff_expiry".to_owned(),
			columns: vec!["expires_at".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "desktop_refresh_credentials".to_owned(),
			name: "desktop_refresh_session".to_owned(),
			columns: vec!["session_id".to_owned()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_reverse_context.sql").to_owned(),
			reverse_sql: Some(include_str!("sql/backward/baseline_reverse_context.sql").to_owned()),
		})
}
