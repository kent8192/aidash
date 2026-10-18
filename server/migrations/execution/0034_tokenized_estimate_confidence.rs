// reinhardt-migration-source: 1
// A Token Encoding labels its Request Estimates `tokenized` (ADR 0017, revised
// 2026-10-17); the unused `exact` confidence is gone. The check is enforced
// apart from the ORM state since 0013_inference_usage_model_state, so this
// migration changes the database only.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0034_tokenized_estimate_confidence", "execution")
		.database_only(true)
		.add_dependency("execution", "0033_research_event_run_index")
		.add_operation(Operation::DropConstraintDefinition {
			table: "inference_usage".into(),
			constraint: Constraint::Check {
				name: "inference_usage_estimate_confidence".into(),
				expression: "estimate_confidence IN ('exact', 'calibrated', 'conservative')".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "inference_usage".into(),
			constraint: Constraint::Check {
				name: "inference_usage_estimate_confidence".into(),
				expression: "estimate_confidence IN ('tokenized', 'calibrated', 'conservative')"
					.into(),
			},
		})
		.atomic(true)
}
