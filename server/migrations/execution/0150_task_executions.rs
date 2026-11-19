// reinhardt-migration-source: 1
// The Run is retired (#1009, ADR 0141): its row is the Task's execution record,
// so `runs` becomes `task_executions` (same rows, keys and policies). The ORM
// names its enum-domain checks after the table, so the physical
// `control_source` check is dropped and added under the new name; 0151 does
// the same for the two checks that exist only in the ORM state. Constraint and
// index renames and the function bodies that name the table are SQL: Reinhardt
// has no typed operation for them. Stored `run.*` event kinds become
// `execution.*` (Reinhardt Query output of
// `execution::services::execution_event_backfill`, checked by
// `manage migrationseeds`). Every app whose history names `runs` runs first,
// so reversing one of their migrations reverses this one before it.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0150_task_executions", "execution")
		.add_dependency("execution", "0149_threads_state")
		.add_dependency("identity", "0111_thread_member_policy")
		.add_dependency("knowledge", "0058_threads")
		.add_dependency("federation", "0030_threads")
		.add_dependency("registry", "0081_thread_tools")
		.atomic(true)
		.add_operation(Operation::RenameTable {
			old_name: "runs".into(),
			new_name: "task_executions".into(),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "task_executions".into(),
			constraint: Constraint::EnumDomain {
				name: "runs_control_source_model_enum_check".into(),
				column: "control_source".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("deployment_administrator".into()),
						ModelEnumValue::String("system".into()),
						ModelEnumValue::String("tenant".into()),
					],
				},
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "task_executions".into(),
			constraint: Constraint::EnumDomain {
				name: "task_executions_control_source_model_enum_check".into(),
				column: "control_source".into(),
				domain: FieldDomain::Enum {
					repr: ModelEnumRepr::String,
					values: vec![
						ModelEnumValue::String("deployment_administrator".into()),
						ModelEnumValue::String("system".into()),
						ModelEnumValue::String("tenant".into()),
					],
				},
			},
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0150_task_execution_names.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0150_task_execution_names.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0150_task_execution_functions.sql").into(),
			reverse_sql: Some(
				include_str!("sql/backward/0150_task_execution_functions.sql").into(),
			),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0150_execution_events.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0150_execution_events.sql").into()),
		})
}
