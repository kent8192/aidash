// reinhardt-migration-source: 1
// A journal created before authorization does not establish provider I/O.
// Historical attempts remain NULL (unknown); new Summary attempts start false
// and turn true atomically with the physical provider call's Usage Record.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0051_compaction_provider_dispatch", "execution")
		.add_dependency("execution", "0050_usage_evidence")
		.add_operation(Operation::AddColumn {
			table: "context_compaction_attempts".into(),
			column: ColumnDefinition::new("provider_dispatched", FieldType::Boolean),
			mysql_options: None,
		})
		.atomic(true)
}
