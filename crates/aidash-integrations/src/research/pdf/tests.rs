use super::*;
use rstest::rstest;

/// One page of a test PDF: its raw content stream.
pub(crate) fn text_page(lines: &[&str]) -> String {
	let mut content = String::from("BT /F1 12 Tf 72 720 Td 14 TL");
	for line in lines {
		content.push_str(&format!(" ({}) Tj T*", escape(line)));
	}
	content.push_str(" ET");
	content
}

fn escape(text: &str) -> String {
	text.replace('\\', "\\\\")
		.replace('(', "\\(")
		.replace(')', "\\)")
}

/// A minimal PDF whose pages run the given content streams with Helvetica
/// as `/F1`. Offsets in the cross-reference table are exact.
pub(crate) fn pdf(pages: &[String]) -> Vec<u8> {
	let mut objects = vec![
		"<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
		String::new(),
		"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
			.to_owned(),
	];
	let mut kids = Vec::new();
	for content in pages {
		let page = objects.len() + 1;
		kids.push(format!("{page} 0 R"));
		objects.push(format!(
			"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
			page + 1
		));
		objects.push(format!(
			"<< /Length {} >>\nstream\n{content}\nendstream",
			content.len()
		));
	}
	objects[1] = format!(
		"<< /Type /Pages /Kids [{}] /Count {} >>",
		kids.join(" "),
		pages.len()
	);
	let mut out = b"%PDF-1.7\n".to_vec();
	let mut offsets = Vec::new();
	for (index, object) in objects.iter().enumerate() {
		offsets.push(out.len());
		out.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
	}
	let xref = out.len();
	out.extend_from_slice(
		format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
	);
	for offset in offsets {
		out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
	}
	out.extend_from_slice(
		format!(
			"trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
			objects.len() + 1
		)
		.as_bytes(),
	);
	out
}

#[rstest]
fn lines_carry_their_physical_page_numbers() {
	let text = extract(&pdf(&[
		text_page(&["The first page explains the method.", "It continues here."]),
		text_page(&["The second page reports the results."]),
	]))
	.unwrap();
	assert_eq!(text.page_count, 2);
	assert_eq!(
		text.lines,
		vec![
			Line {
				text: "The first page explains the method.".into(),
				page: Some(1)
			},
			Line {
				text: "It continues here.".into(),
				page: Some(1)
			},
			Line {
				text: "The second page reports the results.".into(),
				page: Some(2)
			},
		]
	);
	assert!(text.pages_without_text.is_empty());
	assert_eq!(text.completeness, Completeness::Complete);
}

#[rstest]
fn positioned_words_are_separated_and_kerning_is_not() {
	// Producers such as TeX position words instead of drawing spaces; a small
	// kerning adjustment inside a word must not split it.
	let content = "BT /F1 12 Tf 72 720 Td [(Evidence) -300 (binds) -250 (to) -300 (its) -280 (Run.)] TJ 0 -14 Td [(W) 80 (orld) -300 (wide) -280 (sources)] TJ ET";
	let text = extract(&pdf(&[content.to_owned()])).unwrap();
	let lines = text
		.lines
		.iter()
		.map(|line| line.text.as_str())
		.collect::<Vec<_>>();
	assert_eq!(lines, ["Evidence binds to its Run.", "World wide sources"]);
}

#[rstest]
fn pages_without_text_are_listed_and_contribute_no_lines() {
	let text = extract(&pdf(&[
		text_page(&["A page with enough readable text."]),
		String::from("q 0.5 g 72 72 468 648 re f Q"),
		text_page(&["- 3 -"]),
	]))
	.unwrap();
	assert_eq!(text.pages_without_text, vec![2, 3]);
	assert!(text.lines.iter().all(|line| line.page == Some(1)));
}

#[rstest]
fn a_pdf_with_no_text_at_all_extracts_no_lines() {
	let text = extract(&pdf(&[String::new()])).unwrap();
	assert!(text.lines.is_empty());
	assert_eq!(text.pages_without_text, vec![1]);
}

#[rstest]
#[case::not_a_pdf(b"<html>not a pdf</html>".to_vec())]
#[case::truncated(pdf(&[text_page(&["cut short"])])[..40].to_vec())]
fn unreadable_documents_are_extraction_failures(#[case] bytes: Vec<u8>) {
	assert_eq!(extract(&bytes), Err(Failure::ExtractionFailed));
}

#[rstest]
fn the_child_reply_round_trips() {
	let text = extract(&pdf(&[text_page(&[
		"Round trip through the child protocol.",
	])]))
	.unwrap();
	let encoded = serde_json::to_vec(&Reply::Text(text.clone())).unwrap();
	assert!(
		matches!(serde_json::from_slice(&encoded).unwrap(), Reply::Text(decoded) if decoded == text)
	);
	let encoded = serde_json::to_vec(&Reply::Failure(Failure::EncryptedDocument)).unwrap();
	assert!(matches!(
		serde_json::from_slice(&encoded).unwrap(),
		Reply::Failure(Failure::EncryptedDocument)
	));
}

#[rstest]
fn other_invocations_are_not_the_child() {
	assert_eq!(child_main(&["aidash".into(), "runserver".into()]), None);
}

// Generated once with Ghostscript's pdfwrite from one line of Helvetica text:
// a user password; an owner password whose permissions clear bit 5; and an
// owner password whose permissions keep it.
const USER_PASSWORD: &[u8] = include_bytes!("fixtures/user-password.pdf");
const NO_EXTRACTION: &[u8] = include_bytes!("fixtures/no-extraction.pdf");
const EXTRACTION_PERMITTED: &[u8] = include_bytes!("fixtures/extraction-permitted.pdf");

#[rstest]
#[case::needs_a_password(USER_PASSWORD, Failure::EncryptedDocument)]
#[case::forbids_extraction(NO_EXTRACTION, Failure::ExtractionNotPermitted)]
fn protected_documents_are_refused(#[case] bytes: &[u8], #[case] failure: Failure) {
	assert_eq!(extract(bytes), Err(failure));
}

/// A decoy security dictionary that permits extraction, placed before the
/// document's real one, does not unlock text the real one forbids.
#[rstest]
fn a_decoy_security_dictionary_does_not_permit_extraction() {
	let header = NO_EXTRACTION
		.iter()
		.position(|&byte| byte == b'\n')
		.unwrap()
		+ 1;
	let mut bytes = NO_EXTRACTION[..header].to_vec();
	bytes.extend_from_slice(
		b"9999 0 obj\n<< /Filter /Standard /V 2 /R 3 /O (decoy) /U (decoy) /P -4 >>\nendobj\n",
	);
	bytes.extend_from_slice(&NO_EXTRACTION[header..]);
	assert_eq!(extract(&bytes), Err(Failure::ExtractionNotPermitted));
}

/// Only a trailer names an encryption dictionary: the same bytes in page
/// text leave an unencrypted document readable.
#[rstest]
fn encrypt_in_page_text_does_not_make_a_document_encrypted() {
	let text = extract(&pdf(&[text_page(&["Trailers name /Encrypt << >> here."])])).unwrap();
	assert_eq!(text.lines[0].text, "Trailers name /Encrypt << >> here.");
}

/// Encrypted fixtures name their security dictionary from a trailer; plain
/// documents name none, even when their text spells `/Encrypt`.
#[rstest]
#[case::plain(pdf(&[text_page(&["Plain text."])]), false)]
#[case::encrypt_in_text(pdf(&[text_page(&["See /Encrypt << >> here."])]), false)]
#[case::extraction_permitted(EXTRACTION_PERMITTED.to_vec(), true)]
#[case::no_extraction(NO_EXTRACTION.to_vec(), true)]
fn only_trailers_name_encryption(#[case] bytes: Vec<u8>, #[case] named: bool) {
	let pdf = Pdf::new(bytes).unwrap();
	assert_eq!(names_encryption(&pdf), named);
}

#[rstest]
fn an_encrypted_document_that_permits_extraction_is_read() {
	let text = extract(EXTRACTION_PERMITTED).unwrap();
	assert_eq!(
		text.lines,
		vec![Line {
			text: "Protected evidence on page one.".into(),
			page: Some(1)
		}]
	);
}

/// A reply within the snapshot bounds fits the child's output cap even when
/// JSON escaping doubles its text.
#[rstest]
fn the_reply_cap_holds_an_escaped_text_at_the_snapshot_bounds() {
	let text = "\\\"".repeat(13);
	let lines = (0..MAX_LINES)
		.map(|n| Line {
			text: text.clone(),
			page: Some(u32::try_from(n % MAX_PAGES).unwrap() + 1),
		})
		.collect::<Vec<_>>();
	assert!(lines.iter().map(|line| line.text.len()).sum::<usize>() <= MAX_SNAPSHOT_BYTES);
	let reply = Reply::Text(PdfText {
		title: Some("\"".repeat(MAX_TITLE_BYTES)),
		page_count: u32::try_from(MAX_PAGES).unwrap(),
		lines,
		pages_without_text: (1..=u32::try_from(MAX_PAGES).unwrap()).collect(),
		completeness: Completeness::TextLimit,
	});
	let serialized = serde_json::to_vec(&reply).unwrap();
	assert!(serialized.len() > 2 * MAX_SNAPSHOT_BYTES + 64 * 1024);
	assert!(serialized.len() <= MAX_REPLY_BYTES);
}

/// Info-dictionary titles without a byte order mark are PDFDocEncoding, which
/// differs from Latin-1 in its typographic punctuation.
#[rstest]
#[case::em_dash_and_quotes(b"\x8dReport\x8e \x84 2026".to_vec(), "\u{201C}Report\u{201D} \u{2014} 2026")]
#[case::bullet_and_euro(b"\x80 Costs in \xa0".to_vec(), "\u{2022} Costs in \u{20AC}")]
#[case::latin_1_letters(b"Caf\xe9".to_vec(), "Caf\u{e9}")]
#[case::utf_16(vec![0xfe, 0xff, 0x20, 0x14], "\u{2014}")]
#[case::utf_8(b"\xef\xbb\xbf\xe2\x80\x94".to_vec(), "\u{2014}")]
fn titles_decode_as_pdf_text_strings(#[case] raw: Vec<u8>, #[case] expected: &str) {
	assert_eq!(decode_text_string(&raw), expected);
}

/// Only text a snapshot keeps makes a page text-bearing: control characters
/// count for nothing, as they become spaces.
#[rstest]
fn a_page_of_control_characters_carries_no_text() {
	assert_eq!(page_text(vec!["\u{1}\u{2}\u{7}".repeat(8)]), None);
	assert_eq!(
		page_text(vec![format!("Quarterly\u{1}report {}", "\u{2}".repeat(20))]),
		None,
		"fifteen characters of text"
	);
	assert_eq!(
		page_text(vec!["Quarterly\u{1}results".into(), " ".into()]),
		Some(vec!["Quarterly results".to_owned()])
	);
}
