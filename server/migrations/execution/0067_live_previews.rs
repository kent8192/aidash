// reinhardt-migration-source: 1
// A Live preview's persisted facts, a Tenant's lowered limits and the
// session-lifetime Egress budget its Web server holds (ADR 0070, ADR 0069).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0067_live_previews", "execution")
		.add_dependency("execution", "0066_previews_state")
		.database_only(true)
		.add_operation(Operation::CreateTable {
			name: "core_live_previews".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("port", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("stop_reason", FieldType::Text),
				ColumnDefinition::new("idle_seconds", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("absolute_seconds", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("started_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("last_request_at", FieldType::TimestampTz)
					.with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("pod_host", FieldType::Text),
				ColumnDefinition::new("termination_confirmed", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".into())),
				ColumnDefinition::new("egress_expires_at", FieldType::TimestampTz),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".into())),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".into())),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "core_live_previews_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "core_live_previews_limits".into(),
					expression: "idle_seconds >= 60 AND absolute_seconds >= idle_seconds".into(),
				},
				Constraint::Check {
					name: "core_live_previews_state".into(),
					expression: "(state IN ('starting', 'running', 'stale') AND stop_reason IS NULL) OR (state = 'stopped' AND stop_reason IS NOT NULL)".into(),
				},
				Constraint::Check {
					name: "core_live_previews_port".into(),
					expression: "port BETWEEN 1024 AND 65535".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateIndex {
			table: "core_live_previews".into(),
			columns: vec!["tenant".into(), "run_id".into(), "started_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_preview_limits".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("idle_seconds", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("absolute_seconds", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".into())),
			],
			constraints: vec![Constraint::ForeignKey {
				name: "core_preview_limits_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			}],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "egress_sessions".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("live_preview_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("principal", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("sent_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
				ColumnDefinition::new("received_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
				ColumnDefinition::new("requests", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("0".into())),
				ColumnDefinition::new("max_sent_bytes", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("max_received_bytes", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("max_requests", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("CURRENT_TIMESTAMP".into())),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "egress_sessions_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "egress_sessions_preview_fkey".into(),
					columns: vec!["live_preview_id".into()],
					referenced_table: "core_live_previews".into(),
					referenced_columns: vec!["id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("session_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "egress_requests".into(),
			constraint: Constraint::ForeignKey {
				name: "egress_requests_session_fkey".into(),
				columns: vec!["session_id".into()],
				referenced_table: "egress_sessions".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::SetNull,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
}
