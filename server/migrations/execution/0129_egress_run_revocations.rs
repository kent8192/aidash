// reinhardt-migration-source: 1
// Run attribution and durable denial of revoked Run credentials (ADR 0136).
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0129_egress_run_revocations", "execution")
		.add_dependency("execution", "0128_remove_generated_summary_state")
		.add_operation(Operation::CreateTable {
			name: "egress_run_revocations".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("run_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("revoked_at", FieldType::TimestampTz).with_not_null(true),
				ColumnDefinition::new("reason", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("stopped_at", FieldType::TimestampTz),
			],
			constraints: vec![
				Constraint::ForeignKey {
					name: "egress_run_revocations_tenant_fkey".into(),
					columns: vec!["tenant".into()],
					referenced_table: "tenants".into(),
					referenced_columns: vec!["name".into()],
					on_delete: ForeignKeyAction::NoAction,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
				Constraint::ForeignKey {
					name: "egress_run_revocations_run_fkey".into(),
					columns: vec!["tenant".into(), "run_id".into()],
					referenced_table: "runs".into(),
					referenced_columns: vec!["tenant".into(), "id".into()],
					on_delete: ForeignKeyAction::Cascade,
					on_update: ForeignKeyAction::NoAction,
					deferrable: None,
				},
			],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("run_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("admission_ms", FieldType::BigInteger),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "egress_scope_connections".into(),
			column: ColumnDefinition::new("settle_ms", FieldType::BigInteger),
			mysql_options: None,
		})
		.atomic(true)
}
