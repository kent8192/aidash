// reinhardt-migration-source: 1
// The descriptor contract is a PostgreSQL function body, which has no typed
// Reinhardt operation. It extends the 0023_research_providers body with the
// research `view` operation (#312, ADR 0053); rollback restores that body. It
// does not delete view descriptors already registered; the NOT VALID registry
// checks then reject their later rewrites.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0034_research_view", "registry")
		.add_dependency("registry", "0033_row_level_security")
		.database_only(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0034_research_view.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0034_research_view.sql").into()),
		})
}
