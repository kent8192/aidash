// reinhardt-migration-source: 1
// Database constraints remain in place; models do not declare these checks or Tenant roots.

use reinhardt::db::migrations::FieldType;
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new(
            "0054_memory_constraint_state",
            "knowledge",
        )
        .add_operation(Operation::DropConstraintDefinition {
            table: "memory_journal_proofs".to_string(),
            constraint: Constraint::ForeignKey {
                name: "memory_journal_proofs_tenant_fkey".to_string(),
                columns: vec!["tenant".to_string()],
                referenced_table: "tenants".to_string(),
                referenced_columns: vec!["name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::NoAction,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "memory_journal_proofs".to_string(),
            constraint: Constraint::Check {
                name: "memory_journal_proofs_valid".to_string(),
                expression: "from_seq > 0 AND through_seq >= from_seq AND jsonb_typeof(research) = 'array'"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "memory_tenant_scope".to_string(),
            constraint: Constraint::ForeignKey {
                name: "memory_scope_tenant_fkey".to_string(),
                columns: vec!["tenant".to_string()],
                referenced_table: "tenants".to_string(),
                referenced_columns: vec!["name".to_string()],
                on_delete: ForeignKeyAction::Cascade,
                on_update: ForeignKeyAction::NoAction,
                deferrable: None,
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "memory_tenant_scope".to_string(),
            constraint: Constraint::Check {
                name: "memory_scope_valid".to_string(),
                expression: "scope IN ('workspace','tenant_agent') AND revision > 0"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "memory_units".to_string(),
            constraint: Constraint::Check {
                name: "memory_unit_provenance".to_string(),
                expression: "jsonb_typeof(entities) = 'array' AND jsonb_typeof(evidence) = 'array' AND jsonb_typeof(links) = 'array' AND (kind NOT IN ('observation','mental_model') OR (verification IN ('unverified','recorded') AND jsonb_array_length(evidence) > 0))"
                    .to_string(),
            },
        })
        .add_operation(Operation::DropConstraintDefinition {
            table: "memory_units".to_string(),
            constraint: Constraint::Check {
                name: "memory_unit_verification".to_string(),
                expression: "verification IN ('unverified','recorded','supported','contradicted')"
                    .to_string(),
            },
        })
        .add_dependency("knowledge", "0053_memory_journal_proofs")
        .atomic(true)
        .with_initial(None)
        .state_only(true)
        .database_only(false)
}
