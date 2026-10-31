// reinhardt-migration-source: 1
// Record which Harness Contract version an Inference Attempt rendered under.
// Historical Usage Records are legacy@1; new dispatches supply their version.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0082_harness_contract", "execution")
		.add_dependency("execution", "0081_lease_recoveries")
		.add_operation(Operation::AddColumn {
			table: "inference_usage".into(),
			column: ColumnDefinition::new("harness_contract", FieldType::Integer)
				.with_not_null(true)
				.with_default(Some("1".into())),
			mysql_options: None,
		})
		// Every new dispatch supplies the pinned version; historical rows keep
		// the populated legacy@1 value without a standing default.
		.add_operation(Operation::AlterColumn {
			table: "inference_usage".into(),
			column: "harness_contract".into(),
			old_definition: Some(
				ColumnDefinition::new("harness_contract", FieldType::Integer)
					.with_not_null(true)
					.with_default(Some("1".into())),
			),
			new_definition: ColumnDefinition::new("harness_contract", FieldType::Integer)
				.with_not_null(true),
			mysql_options: None,
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_usage".into(),
			constraint: Constraint::Check {
				name: "inference_usage_harness_contract".into(),
				expression: "harness_contract > 0".into(),
			},
		})
		.atomic(true)
}
