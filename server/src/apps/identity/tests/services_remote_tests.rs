use super::*;
use crate::registry::digest;
#[rstest::rstest]
fn untrusted_inspections_cannot_substitute_or_omit_executor_definitions() {
	let reference = EntityRef {
		id: "agent".into(),
		version: "1.0.0".into(),
	};
	let entry: crate::registry::Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"Fixture"},"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"Fixture"}})).unwrap();
	let model: crate::registry::Entry = serde_json::from_value(json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"Fixture"},"config":{}})).unwrap();
	let mut inspection: Inspection = serde_json::from_value(json!({"node_id":"aidash://host","authority_digest":digest(&json!({})),"agent":entry,"definitions":[{"entry":reference,"kind":"agent","digest":digest(&serde_json::to_value(&entry).unwrap()),"metadata":entry},{"entry":{"id":"model","version":"1.0.0"},"kind":"model","digest":digest(&serde_json::to_value(&model).unwrap()),"metadata":model}]})).unwrap();
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_ok());
	assert!(
		validate(
			&inspection,
			"aidash://other",
			&reference,
			&Search::default()
		)
		.is_err()
	);
	let original = inspection.clone();
	inspection.definitions.pop();
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
	inspection = original.clone();
	inspection.definitions[1] = inspection.definitions[0].clone();
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
	inspection = original.clone();
	inspection
		.agent
		.description
		.insert("en".into(), "Substituted".into());
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
	inspection = original;
	inspection.definitions[1].digest = format!("sha256:{}", "é".repeat(32));
	assert!(validate(&inspection, "aidash://host", &reference, &Search::default()).is_err());
}
