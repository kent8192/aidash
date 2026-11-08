// reinhardt-migration-source: 1
// ORM state for the same-Tenant flag of a remote read. The physical column is added in 0070.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0071_remote_reads_same_tenant_state", "identity")
		.add_dependency("identity", "0070_remote_reads_same_tenant")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "authorization_run_remote_reads".into(),
			column: ColumnDefinition::new("same_tenant", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".to_owned())),
			mysql_options: None,
		})
}
