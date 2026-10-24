// reinhardt-migration-source: 1
// Logical model state; 0053 and 0055 own the physical integrity constraints.
use reinhardt::db::migrations::prelude::*;
pub(super) fn migration() -> Migration {
	Migration::new("0057_approval_observations_state", "execution")
		.add_dependency("execution", "0056_decision_attempt_provider_reports")
		.state_only(true)
		.atomic(true)
		.add_operation(Operation::CreateTable {
			name: "approval_observer_settings".into(),
			columns: vec![
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("settings", FieldType::Jsonb)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("updated_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "approval_observations".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("approval", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("approval_type", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("run_id", FieldType::Uuid)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("workspace_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("credential_id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("subject", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("frame", FieldType::Jsonb)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("evidence", FieldType::Jsonb)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("settlement", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("worker", FieldType::Uuid)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("lease_revision", FieldType::BigInteger)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("leased_until", FieldType::TimestampTz)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "approval_observation_attempts".into(),
			columns: vec![
				ColumnDefinition::new("id", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("observation", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("decision", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("record", FieldType::Jsonb)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("status", FieldType::Text)
					.with_not_null(true)
					.with_primary_key(false),
				ColumnDefinition::new("answers", FieldType::Jsonb)
					.with_not_null(false)
					.with_primary_key(false),
				ColumnDefinition::new("provider_report", FieldType::Jsonb),
				ColumnDefinition::new("created_at", FieldType::TimestampTz)
					.with_not_null(true)
					.with_primary_key(false),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
		.add_operation(Operation::CreateTable {
			name: "approval_observation_population".into(),
			columns: vec![
				ColumnDefinition::new("approval", FieldType::Uuid)
					.with_not_null(true)
					.with_primary_key(true),
				ColumnDefinition::new("tenant", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("approval_type", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("kind", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("observer_unavailable", FieldType::Jsonb),
				ColumnDefinition::new("run_id", FieldType::Uuid),
				ColumnDefinition::new("settlement", FieldType::Text).with_not_null(true),
				ColumnDefinition::new("created_at", FieldType::TimestampTz).with_not_null(true),
			],
			constraints: vec![],
			without_rowid: None,
			interleave_in_parent: None,
			partition: None,
		})
}
