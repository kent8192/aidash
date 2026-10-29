// reinhardt-migration-source: 1
// Remote citations pin the source Run and H's bounded hold expiration.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0014_remote_file_version_citations", "workspaces")
		.add_dependency("workspaces", "0013_message_file_versions_state")
		.add_operation(Operation::AddColumn {
			table: "message_file_versions".into(),
			column: ColumnDefinition::new("remote_run_id", FieldType::Uuid),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "message_file_versions".into(),
			column: ColumnDefinition::new("remote_expires_at", FieldType::TimestampTz),
			mysql_options: None,
		})
		.atomic(true)
}
