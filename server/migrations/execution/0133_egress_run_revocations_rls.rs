// reinhardt-migration-source: 1
// The Tenant root key lives here, outside the model state, as every Tenant-owned
// table keeps it. Reinhardt migrations do not express row-level policies.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0133_egress_run_revocations_rls", "execution")
		.add_dependency("execution", "0132_egress_run_revocations")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "egress_run_revocations".into(),
			constraint: Constraint::ForeignKey {
				name: "egress_run_revocations_tenant_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0133_egress_run_revocations_rls.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0133_egress_run_revocations_rls.sql").into(),
			),
		})
}
