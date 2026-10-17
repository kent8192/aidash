use super::*;
use rstest::rstest;
use serde_json::json;

#[rstest]
fn names_round_trip_and_unknown_names_are_not_encodings() {
	for encoding in TokenEncoding::ALL {
		assert_eq!(TokenEncoding::from_name(encoding.name()), Some(encoding));
		assert_eq!(
			serde_json::to_value(encoding).unwrap(),
			json!(encoding.name())
		);
	}
	assert_eq!(TokenEncoding::from_name("cl100k_base"), None);
	assert!(serde_json::from_value::<TokenEncoding>(json!("cl100k_base")).is_err());
}

#[rstest]
#[case::known(json!("gemma4"), Some(TokenEncoding::Gemma4))]
#[case::retired(json!("cl100k_base"), None)]
#[case::null(json!(null), None)]
fn a_pinned_name_loads_leniently(#[case] name: Value, #[case] expected: Option<TokenEncoding>) {
	assert_eq!(lenient(name).unwrap(), expected);
}

#[rstest]
#[case::empty("", 8)]
#[case::short("short", 8)]
#[case::ascii(&"word ".repeat(1000), 64)]
#[case::multibyte(&"東京 ".repeat(1000), 64)]
#[case::no_whitespace(&"x".repeat(1000), 64)]
#[case::emoji(&"👩‍👩‍👧‍👦".repeat(100), 7)]
fn chunks_cover_the_text_in_bounded_pieces(#[case] text: &str, #[case] size: usize) {
	let split = chunks(text, size).collect::<Vec<_>>();
	assert_eq!(split.concat(), text);
	assert!(
		split
			.iter()
			.all(|chunk| !chunk.is_empty() && chunk.len() <= size)
	);
	assert_eq!(split.len() <= 1, text.len() <= size);
}

#[rstest]
fn chunks_split_before_whitespace() {
	let text = "alpha beta gamma delta epsilon";
	for chunk in chunks(text, 12).skip(1) {
		assert!(chunk.starts_with(' '), "{chunk:?}");
	}
}

#[cfg(not(feature = "token-encodings"))]
#[rstest]
fn without_the_feature_nothing_is_counted() {
	for encoding in TokenEncoding::ALL {
		assert!(!encoding.available());
		assert_eq!(encoding.count("text"), None);
		assert_eq!(encoding.count_body(&json!({"messages": []})), None);
	}
}

#[cfg(feature = "token-encodings")]
mod engine_tests {
	use super::super::engine::count_uncached;
	use super::*;

	#[derive(serde::Deserialize)]
	struct Fixture {
		name: String,
		text: String,
		o200k_base: usize,
		gemma4: usize,
	}

	fn fixtures() -> Vec<Fixture> {
		serde_json::from_str(include_str!("fixtures.json")).unwrap()
	}

	fn reference(fixture: &Fixture, encoding: TokenEncoding) -> usize {
		match encoding {
			TokenEncoding::O200kBase => fixture.o200k_base,
			TokenEncoding::Gemma4 => fixture.gemma4,
		}
	}

	#[rstest]
	#[case::o200k_base(TokenEncoding::O200kBase)]
	#[case::gemma4(TokenEncoding::Gemma4)]
	fn the_embedded_files_match_their_pinned_digests(#[case] encoding: TokenEncoding) {
		// `Engine::load` refuses a file whose digest differs.
		assert!(encoding.available());
	}

	#[rstest]
	#[case::o200k_base(TokenEncoding::O200kBase)]
	#[case::gemma4(TokenEncoding::Gemma4)]
	fn counts_match_the_reference_implementations(#[case] encoding: TokenEncoding) {
		for fixture in fixtures() {
			let expected = reference(&fixture, encoding);
			assert_eq!(
				count_uncached(encoding, &fixture.text),
				Some(expected),
				"{} under {}",
				fixture.name,
				encoding.name()
			);
		}
	}

	#[rstest]
	#[case::o200k_base(TokenEncoding::O200kBase)]
	#[case::gemma4(TokenEncoding::Gemma4)]
	fn chunked_counts_are_exact_below_the_chunk_and_never_low_above_it(
		#[case] encoding: TokenEncoding,
	) {
		for fixture in fixtures() {
			let expected = reference(&fixture, encoding);
			let counted = encoding.count(&fixture.text).unwrap();
			if fixture.text.len() <= CHUNK_BYTES {
				assert_eq!(counted, expected, "{}", fixture.name);
			} else {
				assert!(
					counted >= expected,
					"{}: {counted} < {expected}",
					fixture.name
				);
				// Splits fall where words start, so pieces cost little extra.
				assert!(
					counted <= expected + expected / 50 + 8,
					"{}: {counted} far above {expected}",
					fixture.name
				);
			}
			assert!(counted <= fixture.text.len(), "{}", fixture.name);
		}
	}

	#[rstest]
	fn special_token_text_counts_as_ordinary_text() {
		// tiktoken's ordinary encoding: the marker is plain text to the provider.
		assert_eq!(TokenEncoding::O200kBase.count("<|endoftext|> hi"), Some(8));
		assert_eq!(TokenEncoding::Gemma4.count("<start_of_turn>user"), Some(8));
	}

	#[rstest]
	fn a_body_counts_message_text_once_decoded_plus_framing() {
		let encoding = TokenEncoding::O200kBase;
		let text = "{\"history\":[\"東京 \\\"quoted\\\"\"]}";
		let tool = json!({"type":"function","function":{"name":"read","description":"Read a file","parameters":{"type":"object"}}});
		let body = json!({
			"messages": [
				{"role":"system","content":"Follow the task."},
				{"role":"user","content":text},
			],
			"tools": [tool],
		});
		let count = |text: &str| encoding.count(text).unwrap();
		assert_eq!(
			encoding.count_body(&body),
			Some(
				2 * MESSAGE_TOKENS
					+ count("system")
					+ count("Follow the task.")
					+ count("user") + count(text)
					+ TOOL_TOKENS + count(&tool.to_string())
			)
		);
	}

	/// Counting time for adversarial text, which sets `CHUNK_BYTES`. Run with
	/// `cargo test -p aidash-domain --features token-encodings --release --
	/// --ignored --nocapture counting_time`.
	#[rstest]
	#[ignore = "benchmark"]
	fn counting_time() {
		let inputs = [
			("spaces", " ".repeat(1 << 20)),
			("newlines", "\n".repeat(1 << 20)),
			("letter", "x".repeat(1 << 20)),
			("digits", "7".repeat(1 << 20)),
			("punctuation", "!?".repeat(1 << 19)),
			("cjk", "東".repeat(1 << 18)),
			("prose", "The agent reads the workspace. ".repeat(1 << 15)),
		];
		for encoding in TokenEncoding::ALL {
			for (name, text) in &inputs {
				for size in [4 << 10, 16 << 10, 64 << 10, 1 << 20] {
					let started = std::time::Instant::now();
					let tokens = chunks(text, size)
						.map(|chunk| count_uncached(encoding, chunk).unwrap())
						.sum::<usize>();
					println!(
						"{} {name} chunk={size} tokens={tokens} {:?}",
						encoding.name(),
						started.elapsed()
					);
				}
			}
		}
	}
}
