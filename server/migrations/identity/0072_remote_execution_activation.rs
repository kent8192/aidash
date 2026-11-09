// reinhardt-migration-source: 1
// Record when the receiving Node confirmed it started a scoped execution, so a
// binding left behind by a refused activation is not read as a running Run.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0072_remote_execution_activation", "identity")
		.add_dependency("identity", "0071_remote_reads_same_tenant_state")
		.add_operation(Operation::AddColumn {
			table: "authorization_remote_execution".into(),
			column: ColumnDefinition::new("activated_at", FieldType::TimestampTz)
				.with_not_null(false),
			mysql_options: None,
		})
		.atomic(true)
}
