// reinhardt-migration-source: 1
// Host Access reservations for first-run administration.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0074_bootstrap_reservations", "identity")
		.add_dependency("identity", "0073_provider_credential_origin")
		.database_only(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "tenants".into(), constraint: Constraint::Check { name: "tenants_root".into(), expression: "length(name) BETWEEN 1 AND 256 AND created_by_kind IN ('identity','deployment_administrator') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND (label IS NULL OR length(label) BETWEEN 1 AND 256)".into() },
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tenants".into(), constraint: Constraint::Check { name: "tenants_root".into(), expression: "length(name) BETWEEN 1 AND 256 AND created_by_kind IN ('identity','deployment_administrator','host_access') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND (label IS NULL OR length(label) BETWEEN 1 AND 256)".into() },
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "nodes".into(), constraint: Constraint::Check { name: "nodes_creator".into(), expression: "created_by_kind IN ('identity','deployment_administrator') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND length(label) BETWEEN 1 AND 64".into() },
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "nodes".into(), constraint: Constraint::Check { name: "nodes_creator".into(), expression: "created_by_kind IN ('identity','deployment_administrator','host_access') AND (created_by_kind <> 'identity' OR created_by_identity IS NOT NULL) AND length(label) BETWEEN 1 AND 64".into() },
		})
		.add_operation(Operation::AddColumn {
			table: "demo_reservations".into(),
			column: ColumnDefinition::new("source", FieldType::Text)
				.with_not_null(true)
				.with_default(Some("'demo_data'".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AlterColumn {
			table: "demo_reservations".into(),
			column: "source".into(),
			old_definition: Some(
				ColumnDefinition::new("source", FieldType::Text)
					.with_not_null(true)
					.with_default(Some("'demo_data'".into())),
			),
			new_definition: ColumnDefinition::new("source", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "demo_reservations".into(), constraint: Constraint::Unique { name: "demo_reservations_tenant_email_key".into(), columns: vec!["tenant".into(), "email".into()] },
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "demo_reservations".into(), constraint: Constraint::Unique { name: "demo_reservations_tenant_email_source_key".into(), columns: vec!["tenant".into(), "email".into(), "source".into()] },
		})
		.add_operation(Operation::CreateTable {
			name: "bootstrap_deployment_administrator_reservations".into(),
			columns: vec![
				ColumnDefinition::new("email", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("issuer", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.atomic(true)
}
