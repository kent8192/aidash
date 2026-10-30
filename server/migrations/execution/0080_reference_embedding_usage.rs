// reinhardt-migration-source: 1
//! Attribute private reference-image embeddings separately from workspace indexing.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0080_reference_embedding_usage", "execution")
		.database_only(true)
		.add_dependency("execution", "0079_run_safety_stops_state")
		.add_operation(Operation::DropConstraintDefinition {
			table: "generation_embedding_usage".into(),
			constraint: Constraint::Check {
				name: "generation_embedding_usage_purpose_check".into(),
				expression: "(purpose = ANY (ARRAY['query'::text, 'index'::text]))".into(),
			},
		})
		.add_operation(Operation::AddConstraintDefinition {
			table: "generation_embedding_usage".into(),
			constraint: Constraint::Check {
				name: "generation_embedding_usage_purpose_check".into(),
				expression:
					"(purpose = ANY (ARRAY['query'::text, 'index'::text, 'reference'::text]))"
						.into(),
			},
		})
		.atomic(true)
}
