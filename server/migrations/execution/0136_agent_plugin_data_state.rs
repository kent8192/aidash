// reinhardt-migration-source: 1
// Model state for saved Agent Plugin data; 0134 owns the table, its keys and checks.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0136_agent_plugin_data_state", "execution")
		.add_dependency("execution", "0135_agent_plugin_data_rls")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "agent_plugin_data".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("plugin", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("data", FieldType::Binary).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("files", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("sha256", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("saved_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
