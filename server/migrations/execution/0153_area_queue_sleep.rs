// reinhardt-migration-source: 1
// An execution holds its Working Area's queue only while it is awake (#1008,
// ADR 0140 amendment of 2026-11-20). `core_runs.asleep` marks an execution
// that sleeps in a wait that ends its Stint, or is paused: it no longer
// blocks the executions queued behind it, and it re-enters the queue at the
// tail when it wakes (the lease does that). The trigger that marks it and
// tells the next execution of the Area it may run is SQL: Reinhardt has no
// typed operation for a function body or a trigger. The Reinhardt Query
// backfill uses the same eligibility function as the trigger. Reversal drops the mark,
// so every execution of an Area queues behind the first that has not ended,
// as before.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0153_area_queue_sleep", "execution")
		.add_dependency("execution", "0152_stint_leases")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "core_runs".into(),
			column: ColumnDefinition::new("asleep", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0153_area_queue_sleep.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0153_area_queue_sleep.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0153_area_queue_sleep_backfill.sql").into(),
			// Rollback drops the backfilled column.
			reverse_sql: Some("".into()),
		})
}
