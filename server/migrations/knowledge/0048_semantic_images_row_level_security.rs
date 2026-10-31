// reinhardt-migration-source: 1
// Reinhardt has no row policy operations (reinhardt-web#6770).
// Replace 0043 and 0044's existing policies atomically with the shared rendered template;
// reversing the migration restores the original equivalent policy.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0048_semantic_images_row_level_security", "knowledge")
		.add_dependency("knowledge", "0047_image_publications_state")
		.add_dependency("identity", "0028_tenancy_policy_functions")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/backward/0048_semantic_images_row_level_security.sql")
				.to_owned(),
			reverse_sql: Some(
				include_str!("sql/forward/0048_semantic_images_row_level_security.sql").to_owned(),
			),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0048_semantic_images_row_level_security.sql").to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0048_semantic_images_row_level_security.sql").to_owned(),
			),
		})
}
