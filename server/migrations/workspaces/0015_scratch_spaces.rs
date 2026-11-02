// reinhardt-migration-source: 1
// Scratch Spaces belong to a Node; membership and settings cannot cross Tenants.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0015_scratch_spaces", "workspaces")
		.add_dependency("workspaces", "0014_remote_file_version_citations")
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "scratch_spaces".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("name", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("creator", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("last_used_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "scratch_spaces_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Unique {
					name: "scratch_spaces_tenant_id_key".into(),
					columns: vec!["tenant".into(), "id".into()],
				},
				Constraint::ForeignKey {
					name: "scratch_spaces_node_fkey".into(),
					columns: vec!["tenant".into(), "node".into()],
					referenced_table: "nodes".into(),
					referenced_columns: vec!["tenant".into(), "name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "scratch_space_settings".into(),
			columns: vec![
				ColumnDefinition::new("space_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("settings", FieldType::Jsonb).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "scratch_space_settings_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "scratch_space_settings_space_fkey".into(),
					columns: vec!["tenant".into(), "space_id".into()],
					referenced_table: "scratch_spaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "scratch_space_members".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("space_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("subject", FieldType::Text).with_not_null(true),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "scratch_space_members_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "scratch_space_members_space_fkey".into(),
					columns: vec!["tenant".into(), "space_id".into()],
					referenced_table: "scratch_spaces".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::Restrict,
					deferrable: None,
				},
				Constraint::Unique {
					name: "scratch_space_members_subject_key".into(),
					columns: vec!["space_id".into(), "subject".into()],
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
