// reinhardt-migration-source: 1
// A Tenant's own Retention Rules (ADR 0045): periods no longer than the
// Deployment's, stored per Tenant, with the history of every change. A Tenant
// without a row keeps the Deployment's periods; a cleared change keeps no
// periods. Every period is at most ten years.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0020_tenant_retention_rules", "identity")
        .database_only(true)
        .add_dependency("identity", "0019_provider_authorizations")
        .add_operation(Operation::CreateTable {
            name: "tenant_retention_rules".into(),
            columns: vec![
                ColumnDefinition::new("tenant", FieldType::Text).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("snapshot_after_run_seconds", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("snapshot_max_age_seconds", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("updated_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("updated_by", FieldType::Text).with_not_null(true),
            ], constraints: vec![
                Constraint::Check { name: "tenant_retention_rules_periods".into(), expression: "snapshot_after_run_seconds BETWEEN 0 AND 315360000 AND snapshot_max_age_seconds BETWEEN 0 AND 315360000".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateTable {
            name: "tenant_retention_rule_history".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("snapshot_after_run_seconds", FieldType::Integer),
                ColumnDefinition::new("snapshot_max_age_seconds", FieldType::Integer),
                ColumnDefinition::new("changed_at", FieldType::TimestampTz).with_not_null(true),
                ColumnDefinition::new("changed_by", FieldType::Text).with_not_null(true),
            ], constraints: vec![
                Constraint::Check { name: "tenant_retention_rule_history_periods".into(), expression: "(snapshot_after_run_seconds IS NULL) = (snapshot_max_age_seconds IS NULL) AND (snapshot_after_run_seconds IS NULL OR (snapshot_after_run_seconds BETWEEN 0 AND 315360000 AND snapshot_max_age_seconds BETWEEN 0 AND 315360000))".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "tenant_retention_rule_history".into(),
            columns: vec!["tenant".into(), "changed_at".into()],
            unique: false,
            index_type: Some(IndexType::BTree),
            where_clause: None,
            concurrently: false,
            expressions: None,
            mysql_options: None,
            operator_class: None,
        })
        .atomic(true)
}
