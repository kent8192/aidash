// reinhardt-migration-source: 1
// Deployment-wide account coverage outlives Tenant purge. No historical rows
// are guessed for existing attachments: development databases are recreated.
// All service transitions serialize under the Tenant capacity lock. The CHECK
// and partial unique index enforce positive spans and one open interval without
// installing an extension; arbitrary overlapping closed rows are not supported.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0003_coverage_periods", "billing")
		.add_dependency("billing", "0002_constraints")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "billing_coverage_periods".into(),
			columns: vec![
				ColumnDefinition::new("account_id", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("consented_by_administrator", FieldType::Uuid)
					.with_not_null(true),
				ColumnDefinition::new("consented_by_owner", FieldType::Uuid).with_not_null(true),
				ColumnDefinition::new("ends_at", FieldType::TimestampTz).with_not_null(false),
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("starts_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_coverage_periods".into(),
			constraint: Constraint::ForeignKey {
				name: "billing_coverage_periods_tenant_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_coverage_periods".into(),
			constraint: Constraint::ForeignKey {
				name: "billing_coverage_periods_account_id_fkey".into(),
				columns: vec!["account_id".into()],
				referenced_table: "billing_accounts".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_coverage_periods".into(),
			constraint: Constraint::ForeignKey {
				name: "billing_coverage_periods_consented_by_administrator_fkey".into(),
				columns: vec!["consented_by_administrator".into()],
				referenced_table: "dashboard_identities".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_coverage_periods".into(),
			constraint: Constraint::ForeignKey {
				name: "billing_coverage_periods_consented_by_owner_fkey".into(),
				columns: vec!["consented_by_owner".into()],
				referenced_table: "dashboard_identities".into(),
				referenced_columns: vec!["id".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "billing_coverage_periods".into(),
			constraint: Constraint::Check {
				name: "billing_coverage_periods_positive_span".into(),
				expression: "ends_at IS NULL OR ends_at > starts_at".into(),
			},
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "billing_coverage_periods".into(),
			name: "billing_coverage_periods_open".into(),
			columns: vec!["tenant".into()],
			unique: true,
			index_type: Some(IndexType::BTree),
			where_clause: Some("ends_at IS NULL".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "billing_coverage_periods".into(),
			name: "billing_coverage_periods_tenant_starts".into(),
			columns: vec!["tenant".into(), "starts_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::CreateNamedIndex {
			table: "billing_coverage_periods".into(),
			name: "billing_coverage_periods_account".into(),
			columns: vec!["account_id".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
