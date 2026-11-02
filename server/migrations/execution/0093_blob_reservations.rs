// reinhardt-migration-source: 1
// Transfer reservations are not retained bytes and must not be counted twice.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0093_blob_reservations", "execution")
		.add_dependency("execution", "0092_content_object_state")
		.database_only(true)
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "core_quotas".into(),
			column: ColumnDefinition::new("reserved_bytes", FieldType::BigInteger)
				.with_not_null(true)
				.with_default(Some("0".into())),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "core_quotas".into(),
			constraint: Constraint::Check {
				name: "core_quotas_reserved_nonnegative".into(),
				expression: "reserved_bytes >= 0".into(),
			},
		})
}
