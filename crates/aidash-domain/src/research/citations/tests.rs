use super::*;
use rstest::rstest;

fn reference(n: u128) -> EvidenceRef {
	EvidenceRef(Uuid::from_u128(n))
}

#[rstest]
fn references_are_parsed_once_in_first_use_order() {
	let (a, b) = (reference(1), reference(2));
	let text = format!("One [[cite:{b}]], two [[cite:{a}]] and again [[cite:{b}]].");
	let found = citations(&text);
	assert_eq!(found.references, vec![b, a]);
	assert!(found.malformed.is_empty());
	assert_eq!(b.to_string().parse::<EvidenceRef>().unwrap(), b);
}

#[rstest]
#[case("[[cite:ev_not-a-uuid]]")]
#[case("[[cite:00000000-0000-0000-0000-000000000001]]")]
#[case("unterminated [[cite:ev_00000000-0000-0000-0000-000000000001")]
fn malformed_markers_are_reported(#[case] text: &str) {
	let found = citations(text);
	assert!(found.references.is_empty());
	assert_eq!(found.malformed.len(), 1);
}

#[rstest]
fn text_without_markers_has_no_citations() {
	assert!(citations("A plain [[link]] answer.").is_empty());
}

#[rstest]
fn invalid_citations_get_one_correction_then_are_withheld() {
	let known = reference(1);
	let unknown = reference(2);
	let found = citations(&format!(
		"[[cite:{known}]] [[cite:{unknown}]] [[cite:ev_x]]"
	));
	assert_eq!(
		verdict(&found, &[], false),
		Verdict::Correct(vec!["ev_x".into()])
	);
	assert_eq!(
		verdict(&found, &[unknown], false),
		Verdict::Correct(vec!["ev_x".into(), unknown.to_string()])
	);
	assert_eq!(verdict(&found, &[unknown], true), Verdict::Withhold);
	let valid = citations(&format!("[[cite:{known}]]"));
	assert_eq!(verdict(&valid, &[], true), Verdict::Accept);
}

#[rstest]
fn citation_count_is_bounded() {
	let text = (0..=MAX_CITATIONS as u128)
		.map(|n| format!("[[cite:{}]]", reference(n)))
		.collect::<String>();
	let found = citations(&text);
	assert_eq!(found.malformed, vec!["citation limit exceeded".to_string()]);
}
