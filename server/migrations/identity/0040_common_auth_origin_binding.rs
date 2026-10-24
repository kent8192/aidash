// reinhardt-migration-source: 1
// Product-local snapshots have no foreign key into authentication's schema.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0040_common_auth_origin_binding", "identity")
		.add_dependency("identity", "0039_foreign_key_relations_state")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "dashboard_execution_origins".into(),
			column: ColumnDefinition::new("account_id", FieldType::Uuid).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_execution_origins".into(),
			column: ColumnDefinition::new("suspension_epoch", FieldType::BigInteger)
				.with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "dashboard_execution_origins".into(),
			column: ColumnDefinition::new("source_disable_epoch", FieldType::BigInteger)
				.with_not_null(false),
			mysql_options: None,
		})
}
