use super::*;

#[test]
fn full_workspace_context_retains_content_with_minimal_memory_status() {
	let budget = 256;
	let available = workspace_budget(budget).unwrap();
	let semantic = json!("x".repeat(available - 2));
	assert_eq!(serde_json::to_vec(&semantic).unwrap().len(), available);
	let reserved = serde_json::to_vec(&json!({"workspace":semantic,"memory":null}))
		.unwrap()
		.len();
	for status in ["disabled", "no_space"] {
		let memory = bounded_status(status, budget - reserved).unwrap();
		let combined = combine(Some(semantic.clone()), memory, budget)
			.unwrap()
			.unwrap();
		assert_eq!(combined["workspace"], semantic);
		assert_eq!(combined["memory"]["status"], status);
		assert!(serde_json::to_vec(&combined).unwrap().len() <= budget);
	}
}
