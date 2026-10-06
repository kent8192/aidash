use super::*;
#[rstest::rstest]
fn persisted_mcp_errors_are_classified_as_failures() {
	for result in [
		serde_json::json!({"is_error":true,"content":[]}),
		serde_json::json!({"isError":true,"content":[]}),
	] {
		let history = vec![
			serde_json::from_value(serde_json::json!({"kind":"tool","call":{"id":"mcp","name":"plugin_0","arguments":{}},"result":result})).unwrap(),
		];
		assert!(collect_calls(&history, 0)[0].is_error);
	}
}
