// reinhardt-migration-source: 1
// Align logical ORM metadata with the Harness Contract check. The physical
// constraint from 0080 is enforced apart from model state, like the counts and
// estimate-confidence checks of 0013_inference_usage_model_state.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0081_harness_contract_state", "execution")
		.add_dependency("execution", "0080_harness_contract")
		.add_operation(Operation::DropConstraintDefinition {
			table: "inference_usage".into(),
			constraint: Constraint::Check {
				name: "inference_usage_harness_contract".into(),
				expression: "harness_contract > 0".into(),
			},
		})
		.state_only(true)
}
