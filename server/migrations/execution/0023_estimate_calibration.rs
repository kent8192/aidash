// reinhardt-migration-source: 1
// Request Estimates (ADR 0017): Usage Records keep each attempt's byte basis,
// output reservation, ratio and calibration key, and complete media-free
// attempts feed a Deployment-wide sample table with no Run, Tenant or attempt
// identifiers. Records written before this migration keep NULL estimate parts.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0023_estimate_calibration", "execution")
        .database_only(true)
        .add_dependency("execution", "0022_outbound_approval_activation")
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("upstream_model", FieldType::Text),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("byte_basis", FieldType::BigInteger),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("output_reservation", FieldType::BigInteger),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("estimate_ratio_ppm", FieldType::BigInteger),
            mysql_options: None,
        })
        .add_operation(Operation::AddColumn {
            table: "inference_usage".into(),
            column: ColumnDefinition::new("calibration_sample", FieldType::Boolean)
                .with_not_null(true)
                .with_default(Some("false".into())),
            mysql_options: None,
        })
        .add_operation(Operation::AddConstraintDefinition {
            table: "inference_usage".into(),
            constraint: Constraint::Check {
                name: "inference_usage_estimate_parts".into(),
                expression: "byte_basis >= 0 AND output_reservation >= 0 AND estimate_ratio_ppm BETWEEN 1 AND 1000000 AND (NOT calibration_sample OR (upstream_model IS NOT NULL AND byte_basis IS NOT NULL))".into(),
            },
        })
        .add_operation(Operation::CreateTable {
            name: "estimate_calibration_samples".into(),
            columns: vec![
                ColumnDefinition::new("id", FieldType::Uuid).with_primary_key(true).with_not_null(true),
                ColumnDefinition::new("upstream_model", FieldType::Text).with_not_null(true),
                ColumnDefinition::new("projection_version", FieldType::Integer).with_not_null(true),
                ColumnDefinition::new("byte_basis", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("prompt_tokens", FieldType::BigInteger).with_not_null(true),
                ColumnDefinition::new("completed_at", FieldType::TimestampTz).with_not_null(true),
            ], constraints: vec![
                Constraint::Check { name: "estimate_calibration_samples_counts".into(), expression: "projection_version > 0 AND byte_basis > 0 AND prompt_tokens > 0 AND length(upstream_model) BETWEEN 1 AND 512".into() },
            ], without_rowid: None, interleave_in_parent: None, partition: None,
        })
        .add_operation(Operation::CreateIndex {
            table: "estimate_calibration_samples".into(),
            columns: vec!["upstream_model".into(), "projection_version".into(), "completed_at".into()],
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
