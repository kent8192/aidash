use super::*;
use rstest::rstest;
use serde_json::json;

#[rstest]
#[case(json!({"kind":"grant","node_id":"aidash://home","execution_node":"aidash://leaf","grant_id":"00000000-0000-0000-0000-000000000001","admission_id":"00000000-0000-0000-0000-000000000002"}))]
#[case(json!({"kind":"admission","node_id":"aidash://leaf","home_node":"aidash://home","grant_id":"00000000-0000-0000-0000-000000000001","admission_id":"00000000-0000-0000-0000-000000000002"}))]
#[case(json!({"kind":"registry","node_id":"aidash://home","id":"agent","version":"1.0.0","digest":"approved"}))]
fn node_qualified_edges_preserve_the_wire_contract(#[case] value: serde_json::Value) {
	// Act
	let reference: Reference = serde_json::from_value(value.clone()).unwrap();
	let input: Input =
		serde_json::from_value(json!({"tenant":"acme","subject":"alice","reference":value}))
			.unwrap();
	// Assert
	assert_eq!(serde_json::to_value(&reference).unwrap(), value);
	assert_eq!(reference.node(), value["node_id"].as_str().unwrap());
	assert_eq!(
		serde_json::to_value(input).unwrap(),
		json!({"tenant":"acme","subject":"alice","reference":value})
	);
	let mut unknown = value;
	unknown["authority_override"] = json!(true);
	assert!(serde_json::from_value::<Reference>(unknown).is_err());
}

#[rstest]
fn peer_input_rejects_unrecognized_authority_fields() {
	let result = serde_json::from_value::<Input>(
		json!({"tenant":"acme","subject":"alice","reference":{"kind":"registry","node_id":"aidash://home","id":"agent","version":"1.0.0","digest":"approved"},"operator":true}),
	);
	assert!(result.is_err());
}

#[rstest]
#[case(true, LIMIT, true, LIMIT)]
#[case(false, LIMIT, false, 0)]
#[case(true, LIMIT+1, false, 0)]
#[case(false, LIMIT+1, false, 0)]
fn denied_or_oversized_frontiers_disclose_no_edges(
	#[case] visible: bool,
	#[case] count: usize,
	#[case] expected_visible: bool,
	#[case] expected_count: usize,
) {
	let edge = Reference::Registry {
		node_id: "aidash://home".into(),
		id: "agent".into(),
		version: "1.0.0".into(),
		digest: "approved".into(),
	};
	let checked = Checked {
		visible,
		pending: vec![edge; count],
	}
	.disclosed();
	assert_eq!(checked.visible, expected_visible);
	assert_eq!(checked.pending.len(), expected_count);
}
