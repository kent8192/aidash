// reinhardt-migration-source: 1
// Catalog Approval restrictions (ADR 0029, ADR 0095): one approval on one Node
// may be limited to named subjects and Assignable Groups. NULL keeps every
// existing approval usable by everyone the Tenant's policy allows; the
// append-only history records the same restriction beside each revision.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0041_catalog_restriction", "identity")
		.add_dependency("identity", "0040_common_auth_origin_binding")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog".into(),
			column: ColumnDefinition::new("restriction", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog_history".into(),
			column: ColumnDefinition::new("restriction", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
}
