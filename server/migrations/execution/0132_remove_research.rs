// reinhardt-migration-source: 1
// Research is no longer a category (#987, ADR 0138). Its tables carry forced
// row policies, composite keys and partial indexes that a typed DropTable
// cannot drop or reverse, and deleting the Web disclosure approval records is
// a data change, so this migration is SQL; 0133 records the model state.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0132_remove_research", "execution")
		.add_dependency("execution", "0131_scope_refusals_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0132_remove_research.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0132_remove_research.sql").into()),
		})
}
