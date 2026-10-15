// reinhardt-migration-source: 1
//! Model state for Request Estimate parts and calibration samples. The physical
//! `calibration_sample` default, checks and lookup index stay database-only.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0024_estimate_calibration_state", "execution")
		.add_dependency("execution", "0023_estimate_calibration")
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
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::CreateTable {
			name: "estimate_calibration_samples".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_primary_key(true)
					.with_not_null(true),
				ColumnDefinition::new("upstream_model", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("projection_version", FieldType::Integer).with_not_null(true),
				ColumnDefinition::new("byte_basis", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("prompt_tokens", FieldType::BigInteger).with_not_null(true),
				ColumnDefinition::new("completed_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.state_only(true)
}
