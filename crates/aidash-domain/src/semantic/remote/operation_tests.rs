use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn operation() -> Operation {
	Operation {
		id: Uuid::from_u128(1),
		home_node: "aidash://home".into(),
		grant_id: Uuid::from_u128(2),
		admission_id: Uuid::from_u128(3),
		boundary: Boundary {
			step: 2,
			input_sequence: 5,
			task_revision: 7,
			inputs_digest: format!("sha256:{}", "c".repeat(64)),
		},
		inputs: vec![InputRead {
			id: Uuid::from_u128(4),
			sequence: 5,
			digest: format!("sha256:{}", "d".repeat(64)),
		}],
		query: "日本語".into(),
		max_tokens: 1024,
		metadata: serde_json::json!({"kind":"memory"}),
	}
}

#[rstest]
fn operation_id_and_digest_retain_the_existing_distinct_wire_canonicalizations(
	mut operation: Operation,
) {
	operation.set_id().unwrap();
	assert_eq!(
		operation.id.to_string(),
		"aae0f890-e2e7-871b-95a8-1dacc5ed1407"
	);
	assert_eq!(
		operation.digest().unwrap(),
		"sha256:70b69522d4e4f2ba8baab0380c0fa178fc80e8ffc883fce4eb7bed4a780f3a88"
	);
	let id = operation.id;
	operation.set_id().unwrap();
	assert_eq!(operation.id, id);
	operation.validate().unwrap();
	operation.boundary.task_revision += 1;
	operation.set_id().unwrap();
	assert_ne!(operation.id, id);
}

#[rstest]
#[case("id",serde_json::json!(Uuid::nil()))]
#[case("grant_id",serde_json::json!(Uuid::nil()))]
#[case("admission_id",serde_json::json!(Uuid::nil()))]
#[case("query",serde_json::json!("  "))]
#[case("query",serde_json::json!("a".repeat(32769)))]
#[case("max_tokens",serde_json::json!(0))]
#[case("max_tokens",serde_json::json!(32769))]
#[case("metadata",serde_json::json!([]))]
#[case("metadata",serde_json::json!({"large":"x".repeat(4096)}))]
fn operation_rejects_unbound_or_unbounded_input(
	operation: Operation,
	#[case] field: &str,
	#[case] value: Value,
) {
	let mut wire = serde_json::to_value(operation).unwrap();
	wire[field] = value;
	let changed: Operation = serde_json::from_value(wire).unwrap();
	assert!(matches!(
		changed.validate(),
		Err(ContractError::Domain(crate::Error::Invalid(_)))
	));
}

#[rstest]
#[case(vec![0])]
#[case(vec![6])]
#[case(vec![2,2])]
#[case(vec![3,2])]
fn consumed_inputs_are_positive_ordered_and_within_the_boundary(
	mut operation: Operation,
	#[case] sequences: Vec<i64>,
) {
	operation.inputs = sequences
		.into_iter()
		.map(|sequence| InputRead {
			sequence,
			id: Uuid::from_u128(99),
			digest: "input".into(),
		})
		.collect();
	assert!(matches!(
		operation.validate(),
		Err(ContractError::Domain(crate::Error::Invalid(_)))
	));
}

#[rstest]
fn unknown_fields_remain_rejected(operation: Operation) {
	let mut wire = serde_json::to_value(operation).unwrap();
	wire["unbound_authority"] = serde_json::json!(true);
	assert!(serde_json::from_value::<Operation>(wire).is_err());
}
