// reinhardt-migration-source: 1
// Logical model state; 0066 owns the physical tables, keys and indexes.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0068_live_previews_state", "execution")
		.add_dependency("execution", "0067_live_previews_row_level_security")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "core_live_previews".into(),
			columns: vec![
				ColumnDefinition::new("absolute_seconds", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("area_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("egress_expires_at", FieldType::TimestampTz),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("idle_seconds", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("last_request_at", FieldType::TimestampTz)
					.with_not_null(true),
				ColumnDefinition::new("pod_host", FieldType::Text),
				ColumnDefinition::new("port", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("started_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("stop_reason", FieldType::Text),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("termination_confirmed", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("true".into())),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "core_preview_limits".into(),
			columns: vec![
				ColumnDefinition::new("absolute_seconds", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("idle_seconds", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "egress_sessions".into(),
			columns: vec![
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("credential_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("live_preview_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("max_received_bytes", FieldType::BigInteger)
					.with_not_null(true),
				ColumnDefinition::new("max_requests", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("max_sent_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("principal", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("received_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
				ColumnDefinition::new("requests", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("0".into())),
				ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("sent_bytes", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("0".into())),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_requests".into(),
			column: ColumnDefinition::new("session_id", FieldType::Uuid)
				.with_not_null(false)
				.with_unique(false)
				.with_primary_key(false)
				.with_auto_increment(false)
				.with_default(None)
				.with_generated(None)
				.with_domain_option(None),
			mysql_options: None,
		})
}
