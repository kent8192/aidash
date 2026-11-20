// reinhardt-migration-source: 1
// The webhook deliveries a Duty Trigger accepted (#1011), so a signed request
// sent again starts nothing. A delivery is the SHA-256 of its body, which the
// signature covers, and the sender's delivery id when it names one; a repeat
// of either is answered with the first delivery's receipt. Rows are kept for
// seven days, go with their Duty and trigger, and are read and written with
// Reinhardt Query, without an ORM model. Keys, checks and indexes are
// database-only; 0064 is the Tenant row policy.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0063_duty_deliveries", "workspaces")
		.add_dependency("workspaces", "0062_task_duty_script")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "duty_deliveries".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("duty_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("trigger_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("delivery_id", FieldType::Text),
				ColumnDefinition::new("event_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("received_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "duty_deliveries_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_deliveries_duty_fkey".into(),
					columns: vec!["tenant".into(), "duty_id".into()],
					referenced_table: "duties".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "duty_deliveries_trigger_fkey".into(),
					columns: vec!["tenant".into(), "trigger_id".into()],
					referenced_table: "duty_triggers".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Unique {
					name: "duty_deliveries_digest_key".into(),
					columns: vec!["tenant".into(), "trigger_id".into(), "digest".into()],
				},
				Constraint::Check {
					name: "duty_deliveries_check".into(),
					expression: "octet_length(digest) = 64 AND (delivery_id IS NULL OR octet_length(delivery_id) BETWEEN 1 AND 256)".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duty_deliveries".into(),
			name: "duty_deliveries_delivery_id_key".into(),
			columns: vec!["tenant".into(), "trigger_id".into(), "delivery_id".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("delivery_id IS NOT NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "duty_deliveries".into(),
			name: "duty_deliveries_received".into(),
			columns: vec!["received_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
