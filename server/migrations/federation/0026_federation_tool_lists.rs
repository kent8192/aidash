// reinhardt-migration-source: 1
//! A Node Registration stores the Federation Tool Request, Allowance and Block of the
//! owning Node towards the registered Node (ADR 0139, #989). A NULL request or allowance
//! means the default list; the block starts empty. Both the registration and its history
//! gain the columns. Aidash is undeployed, so no existing row is rewritten.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	let mut migration = Migration::new("0026_federation_tool_lists", "federation")
		.add_dependency("federation", "0025_federation_token_ids_state")
		.database_only(true)
		.atomic(true);
	for table in ["node_registrations", "node_registration_history"] {
		migration = migration
			.add_operation(Operation::AddColumn {
				table: table.into(),
				column: ColumnDefinition::new("tool_request", FieldType::Jsonb),
				mysql_options: None,
			})
			.add_operation(Operation::AddColumn {
				table: table.into(),
				column: ColumnDefinition::new("tool_allowance", FieldType::Jsonb),
				mysql_options: None,
			})
			.add_operation(Operation::AddColumn {
				table: table.into(),
				column: ColumnDefinition::new("tool_block", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some("'[]'::jsonb".into())),
				mysql_options: None,
			});
	}
	migration
}
