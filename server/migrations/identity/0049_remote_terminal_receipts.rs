// reinhardt-migration-source: 1
//! Terminal Cancel response evidence outlives Node payload and finite catalog.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0049_remote_terminal_receipts","identity")
        .add_dependency("identity","0048_task_origin_source_lifetime_state")
        .database_only(true).atomic(true)
        .add_operation(Operation::CreateTable {
            name:"authorization_remote_terminal_receipts".into(), columns:vec![
        ColumnDefinition::new("id",FieldType::Uuid).with_not_null(true).with_primary_key(true),
        ColumnDefinition::new("tenant",FieldType::Text).with_not_null(true),
        ColumnDefinition::new("node",FieldType::Text).with_not_null(true),
        ColumnDefinition::new("source_node",FieldType::Text).with_not_null(true),
        ColumnDefinition::new("grant_id",FieldType::Uuid).with_not_null(true),
        ColumnDefinition::new("activation",FieldType::Jsonb).with_not_null(true),
    ],
            constraints:vec![
                Constraint::ForeignKey { name:"remote_terminal_receipts_owner_fkey".into(),columns:vec!["node".into(),"tenant".into()],
                    referenced_table:"node_names".into(),referenced_columns:vec!["name".into(),"tenant".into()],
                    on_delete:ForeignKeyAction::NoAction,on_update:ForeignKeyAction::NoAction,deferrable:None },
                Constraint::ForeignKey { name:"remote_terminal_receipts_tenant_fkey".into(),columns:vec!["tenant".into()],
                    referenced_table:"tenants".into(),referenced_columns:vec!["name".into()],
                    on_delete:ForeignKeyAction::NoAction,on_update:ForeignKeyAction::NoAction,deferrable:None },
                Constraint::Unique {name:"remote_terminal_receipts_original_grant".into(),columns:vec!["tenant".into(),"node".into(),"source_node".into(),"grant_id".into()]},
            ],without_rowid:None,interleave_in_parent:None,partition:None,
        })
        // Reinhardt cannot express RLS policies or trigger creation. Only this
        // DDL uses SQL; all application persistence uses supported Query.
        .add_operation(Operation::RunSQL {
            sql:include_str!("sql/forward/0049_remote_terminal_receipts.sql").into(),
            reverse_sql:Some(include_str!("sql/backward/0049_remote_terminal_receipts.sql").into()),
        })
}
