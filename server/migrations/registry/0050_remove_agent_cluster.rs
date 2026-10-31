// reinhardt-migration-source: 1
// PostgreSQL JSONB key removal and PL/pgSQL function bodies have no typed
// Reinhardt migration operation. The fence is disabled only during this atomic
// rewrite because installed definitions otherwise reject every UPDATE.
// SET LOCAL opens the owner transaction's Deployment Scope before accessing
// the forced row-level policies; the native migration executor has no scope API.
// Published Marketplace versions and revisions are immutable: their original
// content/digests can no longer match the migrated Registry copy, so affected
// versions need republication and installations may need replacement. The data
// rewrite is irreversible; it cannot reconstruct the removed references.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0050_remove_agent_cluster", "registry")
		.add_dependency("registry", "0049_plugin_context_merge")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0050_remove_agent_cluster.sql").into(),
			reverse_sql: None,
		})
}
