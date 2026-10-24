// reinhardt-migration-source: 1
//! Permanent cancellation of an admitted receiver grant, before or after activation.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0017_remote_admission_cancellation", "federation")
		.add_dependency("federation", "0016_receiving_keysets_state")
		.database_only(true)
		.add_operation(Operation::AddColumn {
			table: "authorization_remote_admissions".into(),
			column: ColumnDefinition::new("cancelled", FieldType::Boolean)
				.with_not_null(true)
				.with_default(Some("false".into())),
			mysql_options: None,
		})
}
