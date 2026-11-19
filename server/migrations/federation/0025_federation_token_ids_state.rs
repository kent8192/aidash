// reinhardt-migration-source: 1
//! Model state for `0024_federation_token_ids` (#989).
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0025_federation_token_ids_state", "federation")
		.add_dependency("federation", "0024_federation_token_ids")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "federation_token_ids".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("node", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("deployment", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("jti", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("expires_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
