use super::*;
use rstest::rstest;

#[rstest]
fn without_pages_the_text_is_unchanged() {
	// Arrange
	let text = "Summarize the release notes.  ";

	// Act
	let rendered = render(text, &[]).unwrap();

	// Assert
	assert_eq!(rendered, text);
}

#[rstest]
fn pages_are_rendered_in_order_after_the_text() {
	// Arrange
	let pages = vec![
		"https://example.com/notes".to_owned(),
		"http://example.org/a?b=c".to_owned(),
	];

	// Act
	let rendered = render("Summarize them.\n", &pages).unwrap();

	// Assert
	assert_eq!(
		rendered,
		format!(
			"Summarize them.\n\n{URL_INTENT_HEADING}\n- https://example.com/notes\n- http://example.org/a?b=c"
		)
	);
}

#[rstest]
fn pages_alone_render_only_the_instruction() {
	// Act
	let rendered = render("", &["https://example.com/".to_owned()]).unwrap();

	// Assert
	assert_eq!(
		rendered,
		format!("{URL_INTENT_HEADING}\n- https://example.com/")
	);
}

#[rstest]
#[case("ftp://example.com/file")]
#[case("example.com/page")]
#[case("https://user:secret@example.com/")]
#[case("https://example.com/#section")]
#[case("https://example.com/\npath")]
fn addresses_that_cannot_be_read_are_refused(#[case] page: &str) {
	// Act
	let refused = render("Read it.", &[page.to_owned()]);

	// Assert
	assert_eq!(refused, Err(InvalidUrlIntent::Address(page.to_owned())));
}

#[rstest]
fn a_repeated_page_is_refused() {
	// Arrange
	let page = "https://example.com/".to_owned();

	// Act
	let refused = render("Read it.", &[page.clone(), page.clone()]);

	// Assert
	assert_eq!(refused, Err(InvalidUrlIntent::Duplicate(page)));
}

#[rstest]
fn more_than_the_maximum_is_refused() {
	// Arrange
	let pages = (0..=MAX_URL_INTENTS)
		.map(|index| format!("https://example.com/{index}"))
		.collect::<Vec<_>>();

	// Act
	let refused = render("Read them.", &pages);

	// Assert
	assert_eq!(refused, Err(InvalidUrlIntent::TooMany));
}
