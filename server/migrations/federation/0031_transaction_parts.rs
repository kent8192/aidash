// reinhardt-migration-source: 1
//! A coordinator keeps the seed that seals every participant's part (#1107,
//! ADR 0146). Former journals hold whole manifests and no seed, so they are
//! discarded first: Aidash is undeployed, and new cross-Deployment submissions
//! were refused while that shape was current. Deleting rows has no Reinhardt
//! operation, so it is a SQL asset; the reverse cannot restore them.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0031_transaction_parts", "federation")
		.add_dependency("federation", "0030_threads")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0031_transaction_parts.sql").into(),
			reverse_sql: None,
		})
		.add_operation(Operation::AddColumn {
			table: "atomic_coordinators".into(),
			column: ColumnDefinition::new("seed", FieldType::Text).with_not_null(true),
			mysql_options: None,
		})
}
