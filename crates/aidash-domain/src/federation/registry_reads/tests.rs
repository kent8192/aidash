use super::*;
use rstest::rstest;
use serde_json::json;
fn reference() -> Reference {
	Reference {
		entry: EntityRef {
			id: "agent".into(),
			version: "1.0.0".into(),
		},
		digest: format!("sha256:{}", "a".repeat(64)),
	}
}
#[rstest]
#[case::empty(0, false)]
#[case::single(1, true)]
#[case::maximum(128, true)]
#[case::oversized(129, false)]
fn verification_retains_the_reference_count_contract(#[case] count: usize, #[case] allowed: bool) {
	assert_eq!(validate(&vec![reference(); count]).is_ok(), allowed);
}
#[rstest]
#[case::id("id", "invalid id")]
#[case::version("version", "invalid version")]
#[case::prefix("digest", "wrong-digest")]
#[case::short("digest", "sha256:a")]
fn invalid_reference_retains_the_existing_error_boundary(#[case] field: &str, #[case] value: &str) {
	let mut row = reference();
	match field {
		"id" => row.entry.id = value.into(),
		"version" => row.entry.version = value.into(),
		_ => row.digest = value.into(),
	};
	assert!(matches!(validate(&[row]), Err(Error::Invalid(_))));
}
#[rstest]
fn serialized_reference_retains_exact_fields_and_unknown_field_rejection() {
	let row = reference();
	let value = serde_json::to_value(&row).unwrap();
	assert_eq!(
		value,
		json!({"entry":{"id":"agent","version":"1.0.0"},"digest":row.digest})
	);
	assert_eq!(
		serde_json::from_value::<Reference>(value.clone()).unwrap(),
		row
	);
	let mut unknown = value;
	unknown["other"] = json!(true);
	assert!(serde_json::from_value::<Reference>(unknown).is_err());
}
