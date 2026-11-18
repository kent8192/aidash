use super::*;
use aidash_domain::research::document::{Completeness, ExtractedPage, Line};
use async_trait::async_trait;

struct Fixture {
	lines: usize,
	pages_without_text: Vec<u32>,
	/// The text of every line; numbered filler when `None`.
	line: Option<String>,
}

#[async_trait]
impl PageProvider for Fixture {
	fn admit(&self, url: &Url) -> std::result::Result<(), Failure> {
		if url.host_str() == Some("internal") {
			Err(Failure::NonPublicDestination)
		} else {
			Ok(())
		}
	}
	async fn fetch(&self, url: &Url) -> std::result::Result<ExtractedPage, Failure> {
		let paginated = !self.pages_without_text.is_empty();
		Ok(ExtractedPage {
			url: url.to_string(),
			title: Some("Fixture".into()),
			media_type: if paginated {
				"application/pdf"
			} else {
				"text/html"
			}
			.into(),
			fetched_at: chrono::Utc::now(),
			raw_digest: format!("sha256:{}", "0".repeat(64)),
			completeness: Completeness::Complete,
			lines: (0..self.lines)
				.map(|index| Line {
					text: self
						.line
						.clone()
						.unwrap_or_else(|| format!("{index:04} {}", "x".repeat(1000))),
					page: paginated.then_some(1),
				})
				.collect(),
			page_count: paginated.then_some(1 + self.pages_without_text.len() as u32),
			pages_without_text: self.pages_without_text.clone(),
			original: paginated.then(|| b"%PDF".to_vec()),
		})
	}
}

#[tokio::test]
async fn a_long_page_is_read_in_parts_and_a_missing_part_is_refused() {
	let provider = Fixture {
		lines: 40,
		pages_without_text: vec![],
		line: None,
	};
	let first = read(&provider, &json!({"url":"https://example.com/a"}))
		.await
		.unwrap();
	assert_eq!(first["status"], "ok");
	assert_eq!(first["data"]["part"], 1);
	let parts = first["data"]["parts"].as_u64().unwrap();
	assert!(parts > 1);
	assert!(first["data"]["text"].as_str().unwrap().len() <= MAX_PART_BYTES);
	let last = read(
		&provider,
		&json!({"url":"https://example.com/a","part":parts}),
	)
	.await
	.unwrap();
	assert_eq!(last["status"], "ok");
	let beyond = read(
		&provider,
		&json!({"url":"https://example.com/a","part":parts + 1}),
	)
	.await
	.unwrap();
	assert_eq!(beyond["error"]["code"], "no_such_part");
}

#[tokio::test]
async fn pages_without_text_are_listed_and_bad_input_is_refused() {
	let provider = Fixture {
		lines: 1,
		pages_without_text: vec![2, 3],
		line: None,
	};
	let read_pdf = read(&provider, &json!({"url":"https://example.com/a.pdf"}))
		.await
		.unwrap();
	assert_eq!(read_pdf["data"]["pages_without_text"], json!([2, 3]));
	for input in [
		json!({"url":"ftp://example.com/"}),
		json!({"url":"https://example.com/","part":0}),
		json!({"url":"https://example.com/","document_id":"x"}),
		json!({}),
	] {
		let refused = read(&provider, &input).await.unwrap();
		assert_eq!(refused["error"]["code"], "invalid_input", "{input}");
	}
	let internal = read(&provider, &json!({"url":"https://internal/x"}))
		.await
		.unwrap();
	assert_eq!(internal["error"]["code"], "non_public_destination");
	assert_eq!(
		page_host(&json!({"url":"https://docs.example.com/a"})).as_deref(),
		Some("docs.example.com")
	);
	assert_eq!(page_host(&json!({"query":"x"})), None);
}

#[rstest::rstest]
#[case::a_line_of_backslashes(1, "\\".repeat(16 * 1024 - 1))]
#[case::lines_of_quotes(40, "\"".repeat(1000))]
#[case::short_lines_with_tabs(4000, "\t".into())]
#[tokio::test]
async fn every_part_of_escaped_text_fits_the_result_bound(
	#[case] lines: usize,
	#[case] line: String,
) {
	// Arrange: text whose JSON encoding is about twice its length.
	let provider = Fixture {
		lines,
		pages_without_text: vec![],
		line: Some(line.clone()),
	};
	let url = format!("https://example.com/{}", "a".repeat(2000));

	// Act
	let first = read(&provider, &json!({"url":url})).await.unwrap();
	let parts = first["data"]["parts"].as_u64().unwrap();
	let mut text = String::new();
	for part in 1..=parts {
		let result = read(&provider, &json!({"url":url,"part":part}))
			.await
			.unwrap();

		// Assert
		assert_eq!(result["status"], "ok", "part {part}: {}", result["error"]);
		assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_RESULT_BYTES);
		text.push_str(result["data"]["text"].as_str().unwrap());
	}
	assert_eq!(text, format!("{line}\n").repeat(lines));
}
