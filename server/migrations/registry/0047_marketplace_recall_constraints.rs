// reinhardt-migration-source: 1
// The digest check is a PostgreSQL constraint, not model metadata. Keep its
// database-only operation out of makemigrations' model-state comparison, as
// with the other Registry domain constraints. No raw SQL is required.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0047_marketplace_recall_constraints", "registry")
		.add_dependency("registry", "0046_marketplace_recalls")
		.database_only(true)
		.add_operation(Operation::AddConstraintDefinition {
			table: "marketplace_plugin_recalls".into(),
			constraint: Constraint::Check {
				name: "marketplace_plugin_recalls_digest".into(),
				expression: "digest ~ '^sha256:[0-9a-f]{64}$'".into(),
			},
		})
		.atomic(true)
}
