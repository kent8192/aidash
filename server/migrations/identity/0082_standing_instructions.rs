// reinhardt-migration-source: 1
// Tenant Instructions and Node Instructions (#1010): one text per Tenant and per
// Node, each with a revision for optimistic updates.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0082_standing_instructions", "identity")
		.add_dependency("identity", "0081_scheduled_executions")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "tenant_instructions".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("text", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_by", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "tenant_instructions_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "tenant_instructions_valid".into(),
					expression: "revision > 0 AND octet_length(text) <= 16384".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "node_instructions".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("text", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("updated_by", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "node_instructions_node_fkey".into(),
					columns: vec!["tenant".into(), "node".into()],
					referenced_table: "nodes".into(),
					referenced_columns: vec!["tenant".into(), "name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "node_instructions_valid".into(),
					expression: "revision > 0 AND octet_length(text) <= 16384".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
