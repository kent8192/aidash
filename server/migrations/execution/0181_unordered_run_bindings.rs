// reinhardt-migration-source: 1
// The retained capability binding no longer orders Runs or scopes them by generation.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0181_unordered_run_bindings", "execution")
		.add_dependency("execution", "0180_unorder_runs")
		.atomic(true)
		.add_operation(Operation::DropConstraintDefinition {
			table: "core_runs".into(),
			constraint: Constraint::Unique {
				name: "core_run_order".into(),
				columns: vec!["area_id".into(), "sequence".into()],
			},
		})
		.add_operation(Operation::RunSQL {
			sql: "".into(),
			// Reinhardt's AlterColumn does not render a NOT NULL change.
			reverse_sql: Some(
				"ALTER TABLE public.core_runs ALTER COLUMN sequence SET NOT NULL;".into(),
			),
		})
		.add_operation(Operation::RunSQL {
			sql: "".into(),
			// A rollback reconstructs an order from the retained Run IDs.
			reverse_sql: Some(include_str!("sql/backward/0181_unordered_run_bindings.sql").into()),
		})
		.add_operation(Operation::DropColumn {
			table: "core_runs".into(),
			column: "sequence".into(),
			old_definition: Some(ColumnDefinition::new("sequence", FieldType::BigInteger)),
		})
		.add_operation(Operation::DropColumn {
			table: "core_runs".into(),
			column: "generation".into(),
			old_definition: Some(
				ColumnDefinition::new("generation", FieldType::BigInteger)
					.with_not_null(true)
					.with_default(Some("1".into())),
			),
		})
		.add_operation(Operation::CreateIndex {
			table: "core_runs".into(),
			columns: vec!["area_id".into(), "tenant".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
}
