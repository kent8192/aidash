// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0166_artifact_file_retention", "execution")
		.add_dependency("execution", "0165_setting_requests_state")
		.atomic(true)
		.database_only(true)
		// Reinhardt has no trigger operation. Release the hold on Artifact deletion,
		// as 0061 does for message citations.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0166_artifact_file_retention.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0166_artifact_file_retention.sql").into()),
		})
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_retention_holds".into(),
			constraint: Constraint::Check {
				name: "retention_holder_kind".into(),
				expression: "holder_kind IN ('message', 'live_preview', 'federation')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_retention_holds".into(),
			constraint: Constraint::Check {
				name: "retention_holder_kind".into(),
				expression: "holder_kind IN ('message', 'live_preview', 'federation', 'artifact')".into(),
			},
		})
}
