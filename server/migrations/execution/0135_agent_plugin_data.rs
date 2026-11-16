// reinhardt-migration-source: 1
// The saved `${PLUGIN_DATA}` of an Agent's Agent Plugin (ADR 0073): one current version per
// Tenant, Scratch Space, Agent and plugin, replaced whole after its check, and deleted with its
// Scratch Space. `data` packs the files; `size` is their combined bytes, capped at 64 MiB, and
// `sha256` is the digest of `data`.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0135_agent_plugin_data", "execution")
		.add_dependency("execution", "0134_egress_run_revocations_rls")
		.add_dependency("workspaces", "0019_scratch_spaces")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "agent_plugin_data".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("scratch_space_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("plugin", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("data", FieldType::Bytea).with_not_null(true),
				ColumnDefinition::new("size", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("files", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("sha256", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("saved_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "agent_plugin_data_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "agent_plugin_data_scratch_space_fkey".into(),
					columns: vec!["tenant".into(), "scratch_space_id".into()],
					referenced_table: "scratch_spaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Unique {
					name: "agent_plugin_data_owner_key".into(),
					columns: vec![
						"tenant".into(),
						"scratch_space_id".into(),
						"agent_id".into(),
						"plugin".into(),
					],
				},
				Constraint::Check {
					name: "agent_plugin_data_bounds".into(),
					expression: "size BETWEEN 0 AND 67108864 AND files BETWEEN 0 AND 4096 \
						AND octet_length(data) <= 75497472 AND sha256 ~ '^[0-9a-f]{64}$'"
						.into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
