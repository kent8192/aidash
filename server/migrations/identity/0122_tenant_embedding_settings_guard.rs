// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0122_tenant_embedding_settings_guard", "identity")
		.add_dependency("identity", "0121_tenant_embedding_settings_rls")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "tenant_embedding_settings".into(),
			constraint: Constraint::Check {
				name: "tenant_embedding_settings_revision".into(),
				expression: "revision > 0".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "tenant_embedding_settings".into(),
			constraint: Constraint::ForeignKey {
				name: "tenant_embedding_settings_tenant_root_fkey".into(),
				columns: vec!["tenant".into()],
				referenced_table: "tenants".into(),
				referenced_columns: vec!["name".into()],
				on_delete: ForeignKeyAction::Cascade,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
}
