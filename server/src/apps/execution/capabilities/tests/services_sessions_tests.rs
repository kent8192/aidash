//! Unit tests for services::sessions.
use super::received_scope_matches;

#[rstest::rstest]
fn received_scope_allows_delegation_ancestors_and_checks_both_endpoints() {
	assert!(received_scope_matches(
		&[
			"alice".into(),
			"aidash://source/parent@1.0.0".into(),
			"aidash://target/reader@1.1.0".into(),
		],
		"alice",
		"aidash://target/reader@1.1.0",
	));
	assert!(!received_scope_matches(
		&["mallory".into(), "aidash://target/reader@1.1.0".into()],
		"alice",
		"aidash://target/reader@1.1.0",
	));
	assert!(!received_scope_matches(
		&["alice".into(), "aidash://target/other@1.1.0".into()],
		"alice",
		"aidash://target/reader@1.1.0",
	));
}
