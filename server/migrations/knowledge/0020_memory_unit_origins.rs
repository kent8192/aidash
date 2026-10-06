// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0020_memory_unit_origins", "knowledge")
        .add_dependency("knowledge", "0019_remote_memory_reads")
        .add_operation(Operation::CreateTable {
            name:"memory_unit_origins".into(),
            columns:vec![
                ColumnDefinition::new("unit_id",FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("revision",FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("authority",FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("origin_run",FieldType::Uuid),
            ], constraints:vec![
                Constraint::Check {name:"memory_origin_revision".into(),expression:"revision > 0".into()},
                Constraint::ForeignKey {name:"memory_origin_unit".into(),columns:vec!["unit_id".into()],referenced_table:"memory_units".into(),referenced_columns:vec!["id".into()],on_delete:ForeignKeyAction::Restrict,on_update:ForeignKeyAction::Restrict,deferrable:None},
                Constraint::ForeignKey {name:"memory_origin_run".into(),columns:vec!["origin_run".into()],referenced_table:"runs".into(),referenced_columns:vec!["id".into()],on_delete:ForeignKeyAction::Restrict,on_update:ForeignKeyAction::Restrict,deferrable:None},
            ],without_rowid:None,interleave_in_parent:None,partition:None,
        })
        // Statement trigger DDL is unsupported by the typed migration API.
        .add_operation(Operation::RunSQL {sql:"CREATE TRIGGER memory_unit_origins_atomic BEFORE INSERT OR UPDATE OR DELETE ON memory_unit_origins FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard();".into(), reverse_sql:Some("DROP TRIGGER memory_unit_origins_atomic ON memory_unit_origins;".into())})
        .atomic(true)
}
