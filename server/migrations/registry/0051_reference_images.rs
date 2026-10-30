// reinhardt-migration-source: 1
// The private row and its vectors share the Registry transaction and Tenant policy.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0051_reference_images", "registry")
		.add_dependency("registry", "0050_embedding_inputs")
		.add_operation(Operation::AddColumn {
			table: "agent_knowledge".into(),
			column: ColumnDefinition::new("image_index", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
		.add_operation(Operation::AddColumn {
			table: "agent_knowledge".into(),
			column: ColumnDefinition::new("image_data", FieldType::Jsonb).with_not_null(false),
			mysql_options: None,
		})
		// PostgreSQL function bodies cannot be represented by SchemaExpr.
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0051_reference_images.sql").into(),
			reverse_sql: Some(include_str!("sql/backward/0051_reference_images.sql").into()),
		})
		.atomic(true)
}
