use super::*;
use rstest::{fixture, rstest};
use serde_json::json;

#[fixture]
fn envelope() -> Envelope {
	Envelope {
		version: 1,
		node_id: "aidash://home".into(),
		run_id: Uuid::from_u128(1),
		activation_id: Uuid::from_u128(2),
		generation: 7,
	}
}

#[rstest]
#[case::version("version", json!(2), QuarantineReason::UnsupportedVersion)]
#[case::scope("node_id", json!("aidash://other"), QuarantineReason::WrongScope)]
#[case::zero_generation("generation", json!(0), QuarantineReason::WrongScope)]
#[case::negative_generation("generation", json!(-1), QuarantineReason::WrongScope)]
#[case::unknown_field("context", json!({}), QuarantineReason::Malformed)]
#[case::bad_id("run_id", json!("not-a-uuid"), QuarantineReason::Malformed)]
fn invalid_references_cannot_become_executable(
	envelope: Envelope,
	#[case] field: &str,
	#[case] value: serde_json::Value,
	#[case] reason: QuarantineReason,
) {
	// Arrange
	let mut payload = serde_json::to_value(envelope).unwrap();
	payload[field] = value;
	// Act
	let result = Envelope::decode(&serde_json::to_vec(&payload).unwrap(), "aidash://home");
	// Assert
	assert_eq!(result.unwrap_err(), reason);
}

#[rstest]
fn version_rejection_precedes_scope_rejection(mut envelope: Envelope) {
	// Arrange
	envelope.version = 2;
	envelope.node_id = "aidash://other".into();
	envelope.generation = 0;
	// Act
	let result = Envelope::decode(&serde_json::to_vec(&envelope).unwrap(), "aidash://home");
	// Assert
	assert_eq!(result.unwrap_err(), QuarantineReason::UnsupportedVersion);
}

#[rstest]
fn reference_contract_remains_exact(envelope: Envelope) {
	// Arrange
	let expected = json!({"version":1,"node_id":"aidash://home","run_id":Uuid::from_u128(1),"activation_id":Uuid::from_u128(2),"generation":7});
	// Act
	let decoded =
		Envelope::decode(&serde_json::to_vec(&expected).unwrap(), "aidash://home").unwrap();
	// Assert
	assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
	assert_eq!(serde_json::to_value(envelope).unwrap(), expected);
}

#[rstest]
fn obligation_binding_requires_both_run_and_generation(envelope: Envelope) {
	// Arrange
	let mut row = Obligation {
		id: envelope.activation_id,
		run_id: envelope.run_id,
		generation: envelope.generation,
		run_revision: 5,
		state: "pending".into(),
		publication_epoch: 3,
	};
	// Act / Assert
	assert!(row.matches(&envelope));
	row.generation += 1;
	assert!(!row.matches(&envelope));
	row.generation = envelope.generation;
	row.run_id = Uuid::nil();
	assert!(!row.matches(&envelope));
}
