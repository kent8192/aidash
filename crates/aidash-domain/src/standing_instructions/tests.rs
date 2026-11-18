use super::*;

#[test]
fn accepts_the_largest_text() {
	assert!(validate_standing_text(&"a".repeat(MAX_STANDING_INSTRUCTIONS_BYTES)).is_ok());
}

#[test]
fn rejects_a_text_one_byte_too_long() {
	assert!(validate_standing_text(&"a".repeat(MAX_STANDING_INSTRUCTIONS_BYTES + 1)).is_err());
}

#[test]
fn rejects_nul() {
	assert!(validate_standing_text("a\0b").is_err());
}

#[test]
fn accepts_empty_text() {
	assert!(validate_standing_text("").is_ok());
}

#[test]
fn blank_texts_are_empty() {
	let standing = StandingInstructions {
		tenant: "  \n".into(),
		node: String::new(),
	};
	assert!(standing.is_empty());
}

#[test]
fn empty_texts_are_omitted_when_serialized() {
	let json = serde_json::to_value(StandingInstructions::default()).unwrap();
	assert_eq!(json, serde_json::json!({}));
}
