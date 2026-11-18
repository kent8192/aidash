// reinhardt-migration-source: 1
// A Tenant's Retention Rules governed only Document snapshots, which Research
// no longer keeps (#987, ADR 0138). The tables carry forced row policies that
// a typed DropTable cannot drop or reverse, so this is SQL; 0087 records the
// model state.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0086_remove_tenant_retention_rules", "identity")
		.add_dependency("identity", "0085_remove_research_approvals")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0086_remove_tenant_retention_rules.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0086_remove_tenant_retention_rules.sql").into(),
			),
		})
}
