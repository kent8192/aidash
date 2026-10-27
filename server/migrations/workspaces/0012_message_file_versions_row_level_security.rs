// reinhardt-migration-source: 1
// Reinhardt has no row-policy operation; use the repository tenant-policy template.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new(
		"0012_message_file_versions_row_level_security",
		"workspaces",
	)
	.add_dependency("workspaces", "0011_message_file_versions")
	.database_only(true)
	.atomic(true)
	.add_operation(Operation::RunSQL {
		sql: include_str!("sql/forward/0012_message_file_versions_row_level_security.sql")
			.to_owned(),
		reverse_sql: Some(
			include_str!("sql/backward/0012_message_file_versions_row_level_security.sql")
				.to_owned(),
		),
	})
}
