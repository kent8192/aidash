use super::*;

#[test]
fn sub_tags_are_sorted_and_truncated_for_display() {
	let refusal = RouteRefusal {
		tag: "google-vertex/global".into(),
		reasons: vec![RouteReason::SubTags {
			tags: ["z", "d", "b", "a", "c", "e", "f"]
				.into_iter()
				.map(|s| format!("google-vertex/global/{s}"))
				.collect(),
		}],
	};
	let message = refusal.to_string();
	assert!(message.contains("google-vertex/global/a, google-vertex/global/b, google-vertex/global/c, google-vertex/global/d, google-vertex/global/e, and 2 more"), "{message}");
	assert!(!message.contains("google-vertex/global/f"));
	assert!(message.contains("leaf tag"));
}
