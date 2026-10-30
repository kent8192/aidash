// reinhardt-migration-source: 1
// PostgreSQL JSONB CHECK DDL is not expressible with Reinhardt SchemaExpr.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0039_image_index_limits", "knowledge")
		.add_dependency("knowledge", "0038_embedding_inputs")
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0039_image_index_limits.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0039_image_index_limits.sql").into()),
		})
		.atomic(true)
		.database_only(true)
}
