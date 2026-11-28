// reinhardt-migration-source: 1
// An execution binding records its subject; it no longer owns a Working Area ID.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0183_detach_run_area_bindings", "execution")
		.add_dependency("execution", "0182_unpin_live_previews")
		.atomic(true)
		.add_operation(Operation::AddColumn {
			table: "core_runs".into(),
			column: ColumnDefinition::new("scope_subject", FieldType::Text),
			mysql_options: None,
		})
		.add_operation(Operation::RunSQL {
			sql: include_str!("sql/forward/0183_detach_run_area_bindings.sql").into(),
			// Reinhardt's AlterColumn does not render nullability changes.
			reverse_sql: Some(
				"ALTER TABLE public.core_runs ALTER COLUMN area_id SET NOT NULL;".into(),
			),
		})
		.add_operation(Operation::DropNamedIndex {
			table: "core_runs".into(),
			name: "idx_core_runs_area_id_tenant".into(),
			columns: vec!["area_id".into(), "tenant".into()],
			unique: false,
			index_type: Some(IndexType::BTree),
			where_clause: None,
			concurrently: false,
			expressions: None,
			mysql_options: None,
			operator_class: None,
		})
		.add_operation(Operation::RunSQL {
			sql: "".into(),
			// Restore the historical association when downgrading.
			reverse_sql: Some(
				include_str!("sql/backward/0183_detach_run_area_bindings.sql").into(),
			),
		})
		.add_operation(Operation::DropColumn {
			table: "core_runs".into(),
			column: "area_id".into(),
			old_definition: Some(ColumnDefinition::new("area_id", FieldType::Uuid)),
		})
		.add_operation(Operation::RunSQL {
			// Transitional Area readers use a security-invoker view, which
			// Reinhardt's schema operations do not describe.
			sql: include_str!("sql/forward/0183_run_area_lookup.sql").into(),
			reverse_sql: Some("DROP VIEW public.core_run_areas;".into()),
		})
}
