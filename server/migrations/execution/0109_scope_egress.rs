// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0109_scope_egress", "execution")
.add_dependency("execution", "0108_node_preview_merge")
.database_only(true).atomic(true)
.add_operation(Operation::CreateTable {
name: "egress_scope_state".into(),
columns: vec![
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
ColumnDefinition::new("suspended", FieldType::Boolean).with_not_null(true),
ColumnDefinition::new("window_started_at", FieldType::TimestampTz).with_not_null(true),
ColumnDefinition::new("connections_in_window", FieldType::Integer).with_not_null(true),
],
constraints: vec![
Constraint::ForeignKey {
name: "egress_scope_state_tenant_fkey".into(),
columns: vec!["tenant".into()],
referenced_table: "tenants".into(),
referenced_columns: vec!["name".into()],
on_delete: ForeignKeyAction::NoAction,
on_update: ForeignKeyAction::NoAction,
deferrable: None
},
Constraint::ForeignKey {
name: "egress_scope_state_node_fkey".into(),
columns: vec!["tenant".into(), "node".into()],
referenced_table: "nodes".into(),
referenced_columns: vec!["tenant".into(), "name".into()],
on_delete: ForeignKeyAction::Cascade,
on_update: ForeignKeyAction::NoAction,
deferrable: None
},
Constraint::Unique {
name: "egress_scope_state_owner_key".into(),
columns: vec!["tenant".into(), "node".into(), "scope_id".into()]
},
Constraint::Check {
name: "egress_scope_window_nonnegative".into(),
expression: "connections_in_window >= 0".into()
}
],
without_rowid: None,
interleave_in_parent: None,
partition: None
})
.add_operation(Operation::CreateTable {
name: "egress_scope_daily".into(),
columns: vec![
ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("day", FieldType::Text).with_not_null(true),
ColumnDefinition::new("service", FieldType::Text).with_not_null(true),
ColumnDefinition::new("sent_bytes", FieldType::BigInteger).with_not_null(true),
ColumnDefinition::new("received_bytes", FieldType::BigInteger).with_not_null(true),
],
constraints: vec![
Constraint::ForeignKey {
name: "egress_scope_daily_tenant_fkey".into(),
columns: vec!["tenant".into()],
referenced_table: "tenants".into(),
referenced_columns: vec!["name".into()],
on_delete: ForeignKeyAction::NoAction,
on_update: ForeignKeyAction::NoAction,
deferrable: None
},
Constraint::ForeignKey {
name: "egress_scope_daily_scope_fkey".into(),
columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
referenced_table: "egress_scope_state".into(),
referenced_columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
on_delete: ForeignKeyAction::Cascade,
on_update: ForeignKeyAction::NoAction,
deferrable: None
},
Constraint::Unique {
name: "egress_scope_daily_key".into(),
columns: vec!["tenant".into(), "node".into(), "scope_id".into(), "day".into(), "service".into()]
},
Constraint::Check {
name: "egress_scope_daily_nonnegative".into(),
expression: "sent_bytes >= 0 AND received_bytes >= 0".into()
}
],
without_rowid: None,
interleave_in_parent: None,
partition: None
})
.add_operation(Operation::CreateTable {
name: "egress_scope_connections".into(),
columns: vec![
ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
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
constraints: vec![
Constraint::ForeignKey {
name: "egress_scope_connections_tenant_fkey".into(),
columns: vec!["tenant".into()],
referenced_table: "tenants".into(),
referenced_columns: vec!["name".into()],
on_delete: ForeignKeyAction::NoAction,
on_update: ForeignKeyAction::NoAction,
deferrable: None
},
Constraint::ForeignKey {
name: "egress_scope_connections_scope_fkey".into(),
columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
referenced_table: "egress_scope_state".into(),
referenced_columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
on_delete: ForeignKeyAction::Cascade,
on_update: ForeignKeyAction::NoAction,
deferrable: None
},
Constraint::Check {
name: "egress_scope_connection_valid".into(),
expression: "port BETWEEN 1 AND 65535 AND sent_bytes >= 0 AND received_bytes >= 0 AND duration_ms >= 0".into()
}
],
without_rowid: None,
interleave_in_parent: None,
partition: None
})
.add_operation(Operation::CreateTable {
name: "egress_scope_events".into(),
columns: vec![
ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
ColumnDefinition::new("scope_id", FieldType::Uuid).with_not_null(true),
ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
ColumnDefinition::new("delivered_at", FieldType::TimestampTz),
],
constraints: vec![
Constraint::ForeignKey {
name: "egress_scope_events_tenant_fkey".into(),
columns: vec!["tenant".into()],
referenced_table: "tenants".into(),
referenced_columns: vec!["name".into()],
on_delete: ForeignKeyAction::NoAction,
on_update: ForeignKeyAction::NoAction,
deferrable: None
},
Constraint::ForeignKey {
name: "egress_scope_events_scope_fkey".into(),
columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
referenced_table: "egress_scope_state".into(),
referenced_columns: vec!["tenant".into(), "node".into(), "scope_id".into()],
on_delete: ForeignKeyAction::Cascade,
on_update: ForeignKeyAction::NoAction,
deferrable: None
}
],
without_rowid: None,
interleave_in_parent: None,
partition: None
})
}
