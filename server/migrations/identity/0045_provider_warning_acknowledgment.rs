// reinhardt-migration-source: 1
// Persist the exact disclosure accepted with this approval revision.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0045_provider_warning_acknowledgment", "identity")
		.add_dependency("identity", "0044_data_region_restriction")
		.add_operation(Operation::AddColumn {
			table: "authorization_catalog_history".into(),
			column: ColumnDefinition::new("provider_warning_acknowledgment", FieldType::Text)
				.with_not_null(false),
			mysql_options: None,
		})
		.atomic(true)
}
