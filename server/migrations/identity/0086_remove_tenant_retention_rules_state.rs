// reinhardt-migration-source: 1
// ORM state of the removed Tenant Retention Rules; 0083 changed the database.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0086_remove_tenant_retention_rules_state", "identity")
		.add_dependency("identity", "0085_remove_tenant_retention_rules")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::DropTable {
			name: "tenant_retention_rule_history".into(),
		})
		.add_operation(Operation::DropTable {
			name: "tenant_retention_rules".into(),
		})
}
