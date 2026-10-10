// reinhardt-migration-source: 1
// One Usage Record per Inference Attempt. Unknown counts stay NULL.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0012_inference_usage", "execution")
        .add_dependency("execution", "0011_binding_memory_merge")
        .add_operation(Operation::CreateTable {
            name: "inference_usage".into(),
            columns: vec![
                ColumnDefinition::new("attempt_id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("run_id", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("lease_token", FieldType::Uuid).with_not_null(true),
                ColumnDefinition::new("response_epoch", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("model_id", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("model_version", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("projection_version", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("input_tokens", FieldType::BigInteger),
                ColumnDefinition::new("output_tokens", FieldType::BigInteger),
                ColumnDefinition::new("cache_read_tokens", FieldType::BigInteger),
                ColumnDefinition::new("cache_write_tokens", FieldType::BigInteger),
                ColumnDefinition::new("reasoning_tokens", FieldType::BigInteger),
                ColumnDefinition::new("cost_nanocredits", FieldType::BigInteger),
                ColumnDefinition::new("upstream_cost_nanocredits", FieldType::BigInteger),
                ColumnDefinition::new("estimated_tokens", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("estimator", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("estimator_version", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("estimate_confidence", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("cache_identity", FieldType::Text),
                ColumnDefinition::new("outcome", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("rejection_class", FieldType::Text),
                ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true).with_default(Some("now()".into())),
                ColumnDefinition::new("completed_at", FieldType::TimestampTz),
            ], constraints: vec![
                Constraint::ForeignKey { name: "inference_usage_run".into(), columns: vec!["run_id".into()], referenced_table: "runs".into(), referenced_columns: vec!["id".into()], on_delete: ForeignKeyAction::Cascade, on_update: ForeignKeyAction::Restrict, deferrable: None },
                Constraint::Check { name: "inference_usage_estimate_confidence".into(), expression: "estimate_confidence IN ('exact', 'calibrated', 'conservative')".into() },
                Constraint::Check { name: "inference_usage_outcome".into(), expression: "outcome IN ('dispatched', 'completed', 'rejected', 'unknown') AND (outcome = 'dispatched') = (completed_at IS NULL) AND (outcome = 'rejected') = (rejection_class IS NOT NULL)".into() },
                Constraint::Check { name: "inference_usage_counts".into(), expression: "projection_version > 0 AND estimator_version > 0 AND estimated_tokens >= 0 AND input_tokens >= 0 AND output_tokens >= 0 AND cache_read_tokens >= 0 AND cache_write_tokens >= 0 AND reasoning_tokens >= 0 AND cost_nanocredits >= 0 AND upstream_cost_nanocredits >= 0".into() },
                Constraint::Check { name: "inference_usage_reported_only_when_completed".into(), expression: "outcome = 'completed' OR num_nonnulls(input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, cost_nanocredits, upstream_cost_nanocredits) = 0".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "inference_usage".into(),
            columns: vec!["run_id".into(), "outcome".into()],
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
