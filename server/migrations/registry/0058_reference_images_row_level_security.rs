// reinhardt-migration-source: 1
// Reinhardt has no row policy operations (reinhardt-web#6770).
// The policy is installed atomically with the table in 0056. Keep this
// migration as a no-op for databases that have already recorded it.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0058_reference_images_row_level_security", "registry")
		.add_dependency("registry", "0057_reference_image_byte_state")
		.add_dependency("identity", "0028_tenancy_policy_functions")
		.database_only(true)
		.atomic(true)
}
