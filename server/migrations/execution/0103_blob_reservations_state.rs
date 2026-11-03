// reinhardt-migration-source: 1
// Track the distinct reservation counter in the ORM state.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0103_blob_reservations_state", "execution")
		.add_dependency("execution", "0102_blob_reservations")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "core_quotas".into(),
			column: ColumnDefinition::new("reserved_bytes", FieldType::BigInteger)
				.with_not_null(true),
			mysql_options: None,
		})
}
