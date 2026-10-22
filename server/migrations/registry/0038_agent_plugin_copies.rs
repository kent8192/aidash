// reinhardt-migration-source: 1
// The Tenant's stored copies of imported Agent Plugins (#414, ADR 0072). A copy
// is addressed by the tree digest an Agent definition pins and exists before
// the Agent version that references it, so it has no key into `registry`.
// Typed operations express the whole table; the copy is immutable, which the
// repository enforces by never updating a row.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0038_agent_plugin_copies", "registry")
		.database_only(true)
		.add_dependency("registry", "0037_tenant_owned_row_level_security")
		.add_operation(Operation::CreateTable {
			name: "agent_plugin_copies".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("name", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("source", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("plugin", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("files", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("denied", FieldType::Jsonb).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".into())),
			],
			constraints: vec![
				Constraint::PrimaryKey {
					name: "agent_plugin_copies_pkey".into(),
					columns: vec!["tenant".into(), "digest".into()],
				},
				Constraint::ForeignKey {
					name: "agent_plugin_copies_tenant_root_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::Check {
					name: "agent_plugin_copies_digest".into(),
					expression: "digest ~ '^sha256:[0-9a-f]{64}$'".into(),
				},
				Constraint::Check {
					name: "agent_plugin_copies_documents".into(),
					expression: "jsonb_typeof(plugin) = 'object' AND plugin ->> 'digest' = digest AND jsonb_typeof(files) = 'array' AND jsonb_typeof(denied) = 'array'".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.atomic(true)
}
