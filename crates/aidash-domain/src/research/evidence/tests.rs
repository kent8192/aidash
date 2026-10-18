use super::*;
use rstest::rstest;

fn lines(count: usize, text: impl Fn(usize) -> String) -> Vec<Line> {
	(1..=count)
		.map(|number| Line {
			text: text(number),
			page: None,
		})
		.collect()
}

fn texts(lines: &[Line]) -> Vec<&str> {
	lines.iter().map(|line| line.text.as_str()).collect()
}

#[rstest]
fn context_is_the_nearest_whole_lines_outside_the_fragment_in_document_order() {
	// Arrange
	let document = lines(60, |number| format!("line {number}"));

	// Act: the fragment covers lines 30 to 31.
	let context = SnapshotContext::around(&document, 30, 31);

	// Assert
	assert_eq!(context.before.len(), CONTEXT_LINES);
	assert_eq!(context.before.first().unwrap().text, "line 10");
	assert_eq!(context.before.last().unwrap().text, "line 29");
	assert_eq!(context.after.len(), CONTEXT_LINES);
	assert_eq!(context.after.first().unwrap().text, "line 32");
	assert_eq!(context.after.last().unwrap().text, "line 51");
}

#[rstest]
#[case(1, 3, 0, 1)]
#[case(2, 4, 1, 0)]
#[case(1, 4, 0, 0)]
fn context_stops_at_the_ends_of_the_snapshot(
	#[case] line_start: usize,
	#[case] line_end: usize,
	#[case] before: usize,
	#[case] after: usize,
) {
	let document = lines(5, |number| format!("line {number}"));
	let context = SnapshotContext::around(&document[..4], line_start, line_end);
	assert_eq!(context.before.len(), before);
	assert_eq!(context.after.len(), after);
}

#[rstest]
fn the_byte_bound_ends_a_side_at_a_whole_line() {
	// Arrange: 1 KiB lines, so four fit within the 4 KiB bound and a fifth does not.
	let document = lines(30, |number| format!("{number:0>1024}"));

	// Act
	let context = SnapshotContext::around(&document, 15, 15);

	// Assert
	assert_eq!(context.before.len(), 4);
	assert_eq!(context.after.len(), 4);
	assert_eq!(context.before.last().unwrap().text, document[13].text);
	assert_eq!(context.after.first().unwrap().text, document[15].text);
	assert!(
		context
			.before
			.iter()
			.chain(&context.after)
			.all(|line| line.text.len() == 1024),
		"lines are never cut"
	);
}

#[rstest]
fn an_adjacent_line_longer_than_the_byte_bound_leaves_that_side_empty() {
	let mut document = lines(5, |number| format!("line {number}"));
	document[1].text = "x".repeat(CONTEXT_BYTES + 1);
	let context = SnapshotContext::around(&document, 3, 3);
	assert!(context.before.is_empty());
	assert_eq!(texts(&context.after), ["line 4", "line 5"]);
}

fn snapshot(lines: Vec<Line>) -> DocumentSnapshot {
	DocumentSnapshot {
		id: Uuid::new_v4(),
		run_id: Uuid::new_v4(),
		source_id: Uuid::new_v4(),
		requested_url: "https://example.com/start".into(),
		url: "https://example.com/answered".into(),
		title: Some("Answered".into()),
		media_type: "text/html".into(),
		fetched_at: Utc::now(),
		raw_digest: "sha256:raw".into(),
		text_digest: "sha256:text".into(),
		completeness: Completeness::TextLimit,
		lines,
		page_count: None,
		pages_without_text: Vec::new(),
		original: None,
		purged: None,
	}
}

#[rstest]
fn a_cited_fragment_keeps_its_snapshot_provenance() {
	// Arrange
	let document = snapshot(lines(5, |number| format!("line {number}")));
	let passage = Passage {
		text: "line 3".into(),
		line_start: 3,
		line_end: 3,
		pages: vec![],
	};
	let fragment = EvidenceFragment::new(EvidenceRef(Uuid::new_v4()), &document, &passage);

	// Act
	let cited = CitedEvidence::new(fragment.clone(), &document);

	// Assert
	assert_eq!(cited.fragment, fragment);
	assert_eq!(cited.requested_url, "https://example.com/start");
	assert_eq!(cited.url, "https://example.com/answered");
	assert_eq!(cited.title.as_deref(), Some("Answered"));
	assert_eq!(cited.fetched_at, document.fetched_at);
	assert_eq!(cited.completeness, Completeness::TextLimit);
	let context = cited.context.unwrap();
	assert_eq!(texts(&context.before), ["line 1", "line 2"]);
	assert_eq!(texts(&context.after), ["line 4", "line 5"]);
}

#[rstest]
fn a_snapshot_without_retained_text_has_no_context_but_keeps_the_fragment() {
	let mut document = snapshot(lines(3, |number| format!("line {number}")));
	let passage = Passage {
		text: "line 2".into(),
		line_start: 2,
		line_end: 2,
		pages: vec![],
	};
	let fragment = EvidenceFragment::new(EvidenceRef(Uuid::new_v4()), &document, &passage);
	document.lines.clear();
	let cited = CitedEvidence::new(fragment, &document);
	assert_eq!(cited.fragment.text, "line 2");
	assert!(cited.context.is_none());
}

#[rstest]
fn an_empty_passage_is_expired_only_once_its_snapshot_was_purged() {
	// Arrange: a fresh passage whose lines carry no text.
	let mut document = snapshot(lines(2, |_| String::new()));
	let passage = Passage {
		text: String::new(),
		line_start: 1,
		line_end: 1,
		pages: vec![],
	};
	let fragment = EvidenceFragment::new(EvidenceRef(Uuid::new_v4()), &document, &passage);

	// Act + Assert: still retained, so not expired.
	assert!(!CitedEvidence::new(fragment.clone(), &document).expired);

	// The account terms purge the snapshot and empty the passage.
	document.lines.clear();
	document.purged = Some(Purge {
		at: Utc::now(),
		reason: PurgeReason::AccountTerms,
	});
	assert!(CitedEvidence::new(fragment, &document).expired);
}
