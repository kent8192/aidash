// reinhardt-migration-source: 1
// Reinhardt has no row policy operations (reinhardt-web#6770).
// Replace 0053's existing policy atomically with the shared rendered template;
// reversing the migration restores the original equivalent policy.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0056_reference_images_row_level_security", "registry")
		.add_dependency("registry", "0055_reference_image_byte_state")
		.add_dependency("identity", "0028_tenancy_policy_functions")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/backward/0056_reference_images_row_level_security.sql")
				.to_owned(),
			reverse_sql: Some(
				include_str!("sql/forward/0056_reference_images_row_level_security.sql").to_owned(),
			),
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0056_reference_images_row_level_security.sql")
				.to_owned(),
			reverse_sql: Some(
				include_str!("sql/backward/0056_reference_images_row_level_security.sql")
					.to_owned(),
			),
		})
}
