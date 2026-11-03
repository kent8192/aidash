// reinhardt-migration-source: 1
// Logical ORM state: physical foreign keys and uniqueness remain enforced by 0015 and 0017.
// Reinhardt expresses relation state as constraint DDL; this state-only operation never executes SQL.

use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new(
            "0022_scratch_space_model_state",
            "workspaces",
        )
        .add_operation(Operation::DropConstraintDefinition {
            table: "conversations".to_string(),
            constraint: Constraint::ForeignKey {
                name: "conversations_scratch_space_fkey".to_string(),
                columns: vec!["tenant".to_string(), "scratch_space_id".to_string()],
                referenced_table: "scratch_spaces".to_string(),
                referenced_columns: vec!["tenant".to_string(), "id".to_string()],
                on_delete: ForeignKeyAction::NoAction,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "scratch_space_members".to_string(),
            constraint: Constraint::ForeignKey {
                name: "scratch_space_members_space_fkey".to_string(),
                columns: vec!["tenant".to_string(), "space_id".to_string()],
                referenced_table: "scratch_spaces".to_string(),
                referenced_columns: vec!["tenant".to_string(), "id".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "scratch_space_members".to_string(),
            constraint: Constraint::Unique {
                name: "scratch_space_members_subject_key".to_string(),
                columns: vec!["space_id".to_string(), "subject".to_string()],
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "scratch_space_members".to_string(),
            constraint: Constraint::ForeignKey {
                name: "scratch_space_members_tenant_fkey".to_string(),
                columns: vec!["tenant".to_string()],
                referenced_table: "tenants".to_string(),
                referenced_columns: vec!["name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "scratch_space_settings".to_string(),
            constraint: Constraint::ForeignKey {
                name: "scratch_space_settings_space_fkey".to_string(),
                columns: vec!["tenant".to_string(), "space_id".to_string()],
                referenced_table: "scratch_spaces".to_string(),
                referenced_columns: vec!["tenant".to_string(), "id".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "scratch_space_settings".to_string(),
            constraint: Constraint::ForeignKey {
                name: "scratch_space_settings_tenant_fkey".to_string(),
                columns: vec!["tenant".to_string()],
                referenced_table: "tenants".to_string(),
                referenced_columns: vec!["name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "scratch_spaces".to_string(),
            constraint: Constraint::ForeignKey {
                name: "scratch_spaces_node_fkey".to_string(),
                columns: vec!["tenant".to_string(), "node".to_string()],
                referenced_table: "nodes".to_string(),
                referenced_columns: vec!["tenant".to_string(), "name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "scratch_spaces".to_string(),
            constraint: Constraint::ForeignKey {
                name: "scratch_spaces_tenant_fkey".to_string(),
                columns: vec!["tenant".to_string()],
                referenced_table: "tenants".to_string(),
                referenced_columns: vec!["name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::Restrict,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "scratch_spaces".to_string(),
            constraint: Constraint::Unique {
                name: "scratch_spaces_tenant_id_key".to_string(),
                columns: vec!["tenant".to_string(), "id".to_string()],
            },
        })
        .add_operation(Operation::AddConstraint {
            table: "conversations".to_string(),
            constraint_sql: "CONSTRAINT conversations_scratch_space_fkey FOREIGN KEY (tenant, scratch_space_id) REFERENCES scratch_spaces(tenant, id) ON DELETE NO ACTION ON UPDATE NO ACTION"
                .to_string(),
        })
        .add_operation(Operation::AlterColumn {
            table: "scratch_spaces".into(), column: "revision".into(),
            old_definition: Some(ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true).with_default(Some("0".into()))),
            new_definition: ColumnDefinition::new("revision", FieldType::BigInteger).with_not_null(true), mysql_options: None,
        })
        .add_operation(Operation::AlterColumn {
            table: "scratch_spaces".into(), column: "wipe_generation".into(),
            old_definition: Some(ColumnDefinition::new("wipe_generation", FieldType::BigInteger).with_not_null(true).with_default(Some("0".into()))),
            new_definition: ColumnDefinition::new("wipe_generation", FieldType::BigInteger).with_not_null(true), mysql_options: None,
        })
        .add_dependency("workspaces", "0021_conversation_scratch_space")
        .atomic(true)
        .with_initial(None)
        .state_only(true)
}
