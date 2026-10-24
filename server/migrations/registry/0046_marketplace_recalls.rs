// reinhardt-migration-source: 1
// Deployment-owned append-only learned Recalls; no cross-service foreign keys.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0046_marketplace_recalls", "registry")
		.add_dependency("registry", "0045_projection_measurement_config")
		.add_operation(Operation::CreateTable {
			name: "marketplace_plugin_recalls".into(),
			columns: vec![
				ColumnDefinition::new("plugin_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("version", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("digest", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("recalled_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::PrimaryKey {
					name: "marketplace_plugin_recalls_pkey".into(),
					columns: vec!["plugin_id".into(), "version".into()],
				},
				Constraint::Check {
					name: "marketplace_plugin_recalls_digest".into(),
					expression: "digest ~ '^sha256:[0-9a-f]{64}$'".into(),
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddColumn {
			table: "agent_plugin_copies".into(),
			column: ColumnDefinition::new("entitlement_expires_at", FieldType::TimestampTz)
				.with_not_null(false),
			mysql_options: None,
		})
		.atomic(true)
}
