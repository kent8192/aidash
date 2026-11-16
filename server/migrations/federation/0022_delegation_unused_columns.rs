// reinhardt-migration-source: 1
//! A delegation row records only which agent version runs a locally delegated
//! Task (#373). Outbound delivery, its retry, home human requests and the
//! foreign Agent closure that `binding_snapshot` pinned were removed earlier,
//! and nothing writes or reads these columns. Aidash is undeployed, so the
//! reverse restores the columns empty with their former defaults.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0022_delegation_unused_columns", "federation")
		.add_dependency("federation", "0021_node_preview_origin_merge")
		.database_only(true)
		.add_operation(Operation::DropNamedIndex {
			table: "delegations".into(),
			name: "delegations_retry".into(),
			columns: vec!["next_attempt_at".into(), "created_at".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: Some("(NOT delivered)".into()),
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::DropColumn {
			table: "delegations".into(),
			column: "binding_snapshot".into(),
			old_definition: Some(ColumnDefinition::new("binding_snapshot", FieldType::Jsonb)),
		})
		.add_operation(Operation::DropColumn {
			table: "delegations".into(),
			column: "human_requests".into(),
			old_definition: Some(
				ColumnDefinition::new("human_requests", FieldType::Jsonb)
					.with_not_null(true)
					.with_default(Some("'[]'::jsonb".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "delegations".into(),
			column: "next_attempt_at".into(),
			old_definition: Some(
				ColumnDefinition::new("next_attempt_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_default(Some("now()".into())),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "delegations".into(),
			column: "delivered".into(),
			old_definition: Some(
				ColumnDefinition::new("delivered", FieldType::Boolean)
					.with_not_null(true)
					.with_default(Some("false".into())),
			),
		})
}
