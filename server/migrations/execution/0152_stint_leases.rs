// reinhardt-migration-source: 1
// Stints record the worker lease and their Lease Recoveries, and a pause ends a
// Stint (#1009, ADR 0141 amendment of 2026-11-20). `lease_owner` is the last
// holder's claim token and `lease_recoveries` counts the claims that took the
// lease from a holder that never released it. Reinhardt has no trigger
// operation, so replacing `aidash_stint_transition` and widening its trigger
// to `control` and `lease_owner` are the documented raw-SQL exception; the
// reverse restores the 0145 body and trigger before the columns go.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0152_stint_leases", "execution")
		.add_dependency("execution", "0151_task_executions_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "stints".into(),
			column: ColumnDefinition::new("lease_owner", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "stints".into(),
			column: ColumnDefinition::new("lease_recoveries", FieldType::Integer)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0152_stint_leases.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0152_stint_leases.sql").into()),
		})
}
