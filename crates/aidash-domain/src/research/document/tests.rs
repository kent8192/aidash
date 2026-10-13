use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn lines() -> Vec<Line> {
	vec![
		Line {
			text: "Alpha line".into(),
			page: None,
		},
		Line {
			text: "Die Straße <script>inert text</script>".into(),
			page: Some(2),
		},
		Line {
			text: "Omega line".into(),
			page: Some(2),
		},
	]
}

#[rstest]
fn delivery_pages_by_bytes_and_resumes_inside_a_line(lines: Vec<Line>) {
	let first = deliver(&lines, Position::default(), 5).unwrap();
	assert_eq!(first.passage.text, "Alpha");
	assert_eq!((first.passage.line_start, first.passage.line_end), (1, 1));
	let next = first.next.unwrap();
	assert_eq!(next, Position { line: 0, offset: 5 });
	let second = deliver(&lines, next, 24).unwrap();
	assert!(second.passage.text.starts_with(" line\nDie Straße"));
	assert_eq!(second.passage.pages, vec![2]);
	assert_eq!(second.passage.line_start, 1);
	let rest = deliver(&lines, second.next.unwrap(), MAX_DELIVERY_BYTES).unwrap();
	assert!(rest.passage.text.ends_with("Omega line"));
	assert_eq!(rest.next, None);
}

#[rstest]
fn delivery_never_splits_a_character(lines: Vec<Line>) {
	// "Die Straße" puts the two-byte 'ß' across the 9th byte.
	let start = Position { line: 1, offset: 0 };
	let cut = deliver(&lines, start, 9).unwrap();
	assert_eq!(cut.passage.text, "Die Stra");
	let next = deliver(&lines, cut.next.unwrap(), 2).unwrap();
	assert_eq!(next.passage.text, "ß");
	// A budget smaller than the character still delivers it whole.
	let tiny = deliver(&lines, cut.next.unwrap(), 1).unwrap();
	assert_eq!(tiny.passage.text, "ß");
	assert_eq!(tiny.next, next.next);
	assert!(deliver(&lines, cut.next.unwrap(), 0).is_err());
}

#[rstest]
#[case(Position { line: 3, offset: 0 })]
#[case(Position { line: 1, offset: 100 })]
#[case(Position { line: 1, offset: 9 })]
fn forged_positions_are_rejected(lines: Vec<Line>, #[case] position: Position) {
	assert!(deliver(&lines, position, 10).is_err());
}

#[rstest]
fn find_folds_case_and_sharp_s(lines: Vec<Line>) {
	assert_eq!(find(&lines, "STRASSE", false, 0, 10), (vec![1], None));
	assert_eq!(find(&lines, "STRASSE", true, 0, 10), (vec![], None));
	assert_eq!(find(&lines, "line", false, 0, 1), (vec![0], Some(2)));
	assert_eq!(find(&lines, "line", false, 2, 1), (vec![2], None));
	assert_eq!(fold("ẞ"), "ss");
}

#[rstest]
fn cursors_are_bound_to_one_snapshot_operation_and_query(lines: Vec<Line>) {
	let document = Uuid::from_u128(7);
	let digest = text_digest(&lines);
	let binding = find_binding("line", false);
	let cursor = Cursor {
		document,
		digest: digest.clone(),
		operation: Operation::Find,
		position: Position { line: 2, offset: 0 },
		binding: Some(binding.clone()),
	};
	let decoded = Cursor::decode(&cursor.encode()).unwrap();
	assert_eq!(
		decoded
			.check(document, &digest, Operation::Find, Some(&binding))
			.unwrap(),
		Position { line: 2, offset: 0 }
	);
	assert!(
		decoded
			.check(Uuid::from_u128(8), &digest, Operation::Find, Some(&binding))
			.is_err()
	);
	assert!(
		decoded
			.check(document, "sha256:other", Operation::Find, Some(&binding))
			.is_err()
	);
	assert!(
		decoded
			.check(document, &digest, Operation::Fetch, Some(&binding))
			.is_err()
	);
	assert!(
		decoded
			.check(
				document,
				&digest,
				Operation::Find,
				Some(&find_binding("LINE", true))
			)
			.is_err()
	);
	assert!(Cursor::decode("c1.not-base64!").is_err());
	assert!(Cursor::decode(&cursor.encode().replacen("c1.", "c2.", 1)).is_err());
}

#[rstest]
fn extracted_pages_are_bounded_untrusted_input(lines: Vec<Line>) {
	let page = ExtractedPage {
		url: "https://example.com/".into(),
		title: Some("Example".into()),
		media_type: "text/html".into(),
		fetched_at: Utc::now(),
		raw_digest: "sha256:00".into(),
		completeness: Completeness::Complete,
		lines,
	};
	assert!(page.validate().is_ok());
	let mut control = page.clone();
	control.lines[0].text = "bell\u{0007}".into();
	assert!(control.validate().is_err());
	let mut empty = page.clone();
	empty.lines.clear();
	assert!(empty.validate().is_err());
	let mut page_zero = page.clone();
	page_zero.lines[0].page = Some(0);
	assert!(page_zero.validate().is_err());
	let mut local = page.clone();
	local.url = "file:///etc/passwd".into();
	assert!(local.validate().is_err());
	let mut oversized = page;
	oversized.lines = vec![
		Line {
			text: "x".repeat(MAX_LINE_BYTES),
			page: None
		};
		MAX_SNAPSHOT_BYTES / MAX_LINE_BYTES + 1
	];
	assert!(oversized.validate().is_err());
}
