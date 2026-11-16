// reinhardt-migration-source: 1
// Automatically admitted memory is recorded without an unreviewed marker.
use reinhardt::db::migrations::prelude::*;

pub(super) fn migration() -> Migration {
	Migration::new("0051_recorded_memory", "knowledge")
		.add_dependency("knowledge", "0050_recovery_lifetime_merge")
		.add_operation(Operation::DropConstraintDefinition { table: "memory_units".into(), constraint: Constraint::Check { name: "memory_unit_verification".into(), expression: "verification IN ('unverified','supported','contradicted')".into() } })
		.add_operation(Operation::AddConstraintDefinition { table: "memory_units".into(), constraint: Constraint::Check { name: "memory_unit_verification".into(), expression: "verification IN ('unverified','recorded','supported','contradicted')".into() } })
		.add_operation(Operation::DropConstraintDefinition { table: "memory_units".into(), constraint: Constraint::Check { name: "memory_unit_provenance".into(), expression: "jsonb_typeof(entities) = 'array' AND jsonb_typeof(evidence) = 'array' AND jsonb_typeof(links) = 'array' AND (kind NOT IN ('observation','mental_model') OR (verification = 'unverified' AND jsonb_array_length(evidence) > 0))".into() } })
		.add_operation(Operation::AddConstraintDefinition { table: "memory_units".into(), constraint: Constraint::Check { name: "memory_unit_provenance".into(), expression: "jsonb_typeof(entities) = 'array' AND jsonb_typeof(evidence) = 'array' AND jsonb_typeof(links) = 'array' AND (kind NOT IN ('observation','mental_model') OR (verification IN ('unverified','recorded') AND jsonb_array_length(evidence) > 0))".into() } })
}
