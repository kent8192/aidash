// reinhardt-migration-source: 1
// Research is no longer a category (#987, ADR 0138). The Web disclosure
// approval records and their approval observations are deleted first; that
// asset is Reinhardt Query output from
// `execution::services::research_record_cleanup`, checked by
// `manage migrationseeds`. The tables carry forced row policies, composite
// keys and partial indexes that a typed DropTable cannot drop or reverse, so
// dropping them is SQL; 0133 records the model state.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0133_remove_research", "execution")
		.add_dependency("execution", "0132_generation_scheduled")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0133_remove_research_records.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0133_remove_research_records.sql").into()),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0133_remove_research.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0133_remove_research.sql").into()),
		})
}
