// reinhardt-migration-source: 1
// PostgreSQL row-level policies have no typed Reinhardt migration operation.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0121_tenant_embedding_settings_rls", "identity")
		.add_dependency("identity", "0120_tenant_embedding_settings")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0121_tenant_embedding_settings_rls.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0121_tenant_embedding_settings_rls.sql").into(),
			),
		})
}
