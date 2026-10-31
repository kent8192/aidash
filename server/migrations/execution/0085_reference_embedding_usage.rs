// reinhardt-migration-source: 1
//! Attribute private reference-image embeddings separately from workspace indexing.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0085_reference_embedding_usage", "execution")
		.database_only(true)
		.add_dependency("execution", "0081_lease_recoveries")
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
		// PostgreSQL procedural guards are not representable by schema expressions.
		// Put this last so reversal checks the data before restoring the old constraint.
		.add_operation(Operation::RunSQL {
			sql: "SELECT 1".into(),
			reverse_sql: Some("DO $migration$ BEGIN IF EXISTS (SELECT 1 FROM generation_embedding_usage WHERE purpose = 'reference') THEN RAISE EXCEPTION 'reference embedding usage exists; rollback requires a compatible version'; END IF; END $migration$;".into()),
		})
		.atomic(true)
}
