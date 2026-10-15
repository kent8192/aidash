// reinhardt-migration-source: 1
// Issue #243 removes the legacy Marketplace: its packages and the mutable,
// Deployment-wide installation overlays. Triggers have no typed operation, so a
// SQL asset drops them. The tables are dropped by the same asset rather than by
// Operation::DropTable: its reversal renders each column of a composite primary
// key as its own PRIMARY KEY, which PostgreSQL rejects, so the asset recreates
// both tables, their keys and triggers instead. The trigger functions stay;
// nothing calls them once their triggers are gone.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0025_remove_legacy_installations", "registry")
		.database_only(true)
		.add_dependency("registry", "0024_model_provider_keys")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/baseline_search_path.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/baseline_search_path.sql").into()),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "installations".into(),
			constraint: Constraint::ForeignKey {
				name: "installations_id_version_fkey".into(),
				columns: vec!["id".into(), "version".into()],
				referenced_table: "registry".into(),
				referenced_columns: vec!["id".into(), "version".into()],
				on_delete: ForeignKeyAction::NoAction,
				on_update: ForeignKeyAction::NoAction,
				deferrable: None,
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0025_remove_legacy_installations.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0025_remove_legacy_installations.sql").into(),
			),
		})
		.atomic(true)
}
