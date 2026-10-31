// reinhardt-migration-source: 1
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0082_subnodes", "execution")
        .add_dependency("execution", "0081_lease_recoveries")
        .database_only(true).atomic(true)
        .add_operation(Operation::CreateTable {
            name: "subnodes".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_not_null(true).with_primary_key(true),
                ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("node", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("creator_run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("creator_task_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("creator_agent_id", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("creator_agent_version", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("leader_agent_id", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("leader_agent_version", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("allowance", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("workspace_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("title", FieldType::Text),
                ColumnDefinition::new("state", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("ended_at", FieldType::TimestampTz),
                ColumnDefinition::new("ended_reason", FieldType::Text),
                ColumnDefinition::new("creation_key", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("creation_fingerprint", FieldType::Text).with_not_null(true)
            ],
            constraints: vec![Constraint::ForeignKey { name: "subnodes_tenant_fkey".into(), columns: vec!["tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
Constraint::ForeignKey { name: "subnodes_creator_run_fkey".into(), columns: vec!["tenant".into(),"creator_run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["tenant".into(),"id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::NoAction, deferrable: None },
Constraint::ForeignKey { name: "subnodes_workspace_fkey".into(), columns: vec!["tenant".into(),"workspace_id".into()], referenced_table: "workspaces".into(), referenced_columns: vec!["tenant".into(),"id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
Constraint::Unique { name: "subnodes_tenant_id_key".into(), columns: vec!["tenant".into(),"id".into()] },
Constraint::Unique { name: "subnodes_workspace_key".into(), columns: vec!["workspace_id".into()] },
Constraint::Unique { name: "subnodes_creation_key".into(), columns: vec!["creator_run_id".into(),"creation_key".into()] },
Constraint::Check { name: "subnodes_lifecycle".into(), expression: "(state = 'active' AND ended_at IS NULL AND ended_reason IS NULL) OR (state = 'dissolved' AND ended_at IS NOT NULL AND ended_reason IS NOT NULL)".into() }],
            without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "subnode_members".into(),
            columns: vec![
                ColumnDefinition::new("subnode_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("agent_id", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("agent_version", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("added_by_run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("added_at", FieldType::TimestampTz).with_not_null(true)
            ],
            constraints: vec![Constraint::PrimaryKey { name: "subnode_members_pkey".into(), columns: vec!["subnode_id".into(), "agent_id".into()] },
Constraint::ForeignKey { name: "subnode_members_subnode_fkey".into(), columns: vec!["tenant".into(),"subnode_id".into()], referenced_table: "subnodes".into(), referenced_columns: vec!["tenant".into(),"id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::NoAction, deferrable: None },
Constraint::ForeignKey { name: "subnode_members_added_by_fkey".into(), columns: vec!["tenant".into(),"added_by_run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["tenant".into(),"id".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None }],
            without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "subnode_operation_receipts".into(),
            columns: vec![
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("call_key", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("fingerprint", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("result", FieldType::Jsonb).with_not_null(true),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true)
            ],
            constraints: vec![Constraint::PrimaryKey { name: "subnode_operation_receipts_pkey".into(), columns: vec!["run_id".into(), "call_key".into()] },
Constraint::ForeignKey { name: "subnode_operation_receipts_run_fkey".into(), columns: vec!["tenant".into(),"run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["tenant".into(),"id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::NoAction, deferrable: None }],
            without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "subnode_limits".into(),
            columns: vec![
                ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true).with_primary_key(true),
                ColumnDefinition::new("members_per_subnode", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("subnodes_per_run", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("remote_runs_may_create", FieldType::Boolean).with_not_null(true),
                ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("updated_by", FieldType::Text).with_not_null(true)
            ],
            constraints: vec![Constraint::ForeignKey { name: "subnode_limits_tenant_fkey".into(), columns: vec!["tenant".into()], referenced_table: "tenants".into(), referenced_columns: vec!["name".into()], on_delete: ForeignKeyAction::NoAction, on_update: ForeignKeyAction::NoAction, deferrable: None },
Constraint::Check { name: "subnode_limits_range".into(), expression: "members_per_subnode BETWEEN 1 AND 20 AND subnodes_per_run BETWEEN 1 AND 10".into() }],
            without_rowid: None, interleave_in_parent: None, partition: None,
        })
}
