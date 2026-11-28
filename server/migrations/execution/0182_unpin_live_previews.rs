// reinhardt-migration-source: 1
// Live previews follow their Run's Execution Pod.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0182_unpin_live_previews", "execution")
        .add_dependency("execution", "0181_unordered_run_bindings")
        .atomic(true)
        .add_operation(Operation::DropConstraintDefinition {
            table: "core_live_previews".into(),
            constraint: Constraint::ForeignKey {
                name: "core_live_previews_area_fkey".into(),
                columns: vec!["tenant".into(), "area_id".into()],
                referenced_table: "core_areas".into(),
                referenced_columns: vec!["tenant".into(), "id".into()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::NoAction,
                deferrable: None,
            },
        })
        .add_operation(Operation::RunSQL {
            sql: "".into(),
            // Reinhardt's AlterColumn does not render a NOT NULL change.
            reverse_sql: Some(
                "ALTER TABLE public.core_live_previews ALTER COLUMN area_id SET NOT NULL, ALTER COLUMN revision SET NOT NULL;".into(),
            ),
        })
        .add_operation(Operation::RunSQL {
            sql: "".into(),
            // Reconstruct the historical columns from each Run's current binding.
            reverse_sql: Some(include_str!("sql/backward/0182_unpin_live_previews.sql").into()),
        })
        .add_operation(Operation::DropColumn {
            table: "core_live_previews".into(),
            column: "area_id".into(),
            old_definition: Some(ColumnDefinition::new("area_id", FieldType::Uuid)),
        })
        .add_operation(Operation::DropColumn {
            table: "core_live_previews".into(),
            column: "revision".into(),
            old_definition: Some(ColumnDefinition::new("revision", FieldType::BigInteger)),
        })
}
