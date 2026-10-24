use super::*;
#[rstest::rstest]
fn documents_are_bounded_and_nonempty() {
	let mut docs = vec![ReferenceDocument {
		name: "personal.pdf".into(),
		media_type: "application/pdf".into(),
		text: "Reference data".into(),
	}];
	assert!(validate(&docs).is_ok());
	docs[0].text.clear();
	assert!(validate(&docs).is_err());
	docs[0].text = "x".repeat(65537);
	assert!(validate(&docs).is_err());
	assert!(validate(&[]).is_err());
}
