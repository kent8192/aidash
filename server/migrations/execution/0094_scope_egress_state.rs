// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0094_scope_egress_state", "execution")
		.add_dependency("execution", "0093_scope_egress_row_level_security")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "egress_scope_state".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("suspended", FieldType::Boolean).with_not_null(true),
				ColumnDefinition::new("window_started_at", FieldType::TimestampTz)
					.with_not_null(true),
				ColumnDefinition::new("connections_in_window", FieldType::Integer)
					.with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "egress_scope_daily".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("day", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("service", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("sent_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("received_bytes", FieldType::BigInteger).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "egress_scope_connections".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("host", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("port", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("sent_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("received_bytes", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("duration_ms", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("outcome", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "egress_scope_events".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("delivered_at", FieldType::TimestampTz),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
