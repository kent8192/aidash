use super::*;
use std::sync::mpsc;

struct Recorder(mpsc::Sender<InferenceProgress>);

impl InferenceProgressSink for Recorder {
	fn offer(&self, progress: InferenceProgress) {
		self.0.send(progress).unwrap();
	}
}

fn recorder() -> (Recorder, mpsc::Receiver<InferenceProgress>) {
	let (sender, received) = mpsc::channel();
	(Recorder(sender), received)
}

fn sse(chunks: &[Value], done: bool) -> Vec<u8> {
	let mut body = String::from(": OPENROUTER PROCESSING\n\n");
	for chunk in chunks {
		body.push_str(&format!("data: {chunk}\r\n\r\n"));
	}
	if done {
		body.push_str("data: [DONE]\n\n");
	}
	body.into_bytes()
}

fn delta(delta: Value) -> Value {
	json!({"choices":[{"index":0,"delta":delta,"finish_reason":null}]})
}

fn finish(reason: &str) -> Value {
	json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}]})
}

fn usage() -> Value {
	json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":7}})
}

fn assemble(
	bytes: &[u8],
	split: usize,
	limit: usize,
) -> (Result<ModelResponse>, Vec<InferenceProgress>) {
	let (recorder, received) = recorder();
	let mut assembler = StreamAssembler::new(&recorder, limit);
	let mut result = Ok(());
	for chunk in bytes.chunks(split) {
		if let Err(error) = assembler.push(chunk) {
			result = Err(error);
			break;
		}
	}
	let response = result.and_then(|()| assembler.finish());
	(response, received.try_iter().collect())
}

fn same_outcome(streamed: Result<ModelResponse>, whole: Value) {
	match (streamed, parse_openai(whole)) {
		(Ok(streamed), Ok(whole)) => assert_eq!(
			serde_json::to_value(streamed).unwrap(),
			serde_json::to_value(whole).unwrap()
		),
		(Err(streamed), Err(whole)) => assert_eq!(streamed.to_string(), whole.to_string()),
		(streamed, whole) => panic!("streamed {streamed:?} but non-streamed {whole:?}"),
	}
}

#[rstest::rstest]
fn streamed_text_matches_the_non_streamed_completion(#[values(1, 7, 4096)] split: usize) {
	// Arrange
	let body = sse(
		&[
			delta(json!({"role":"assistant","content":""})),
			delta(json!({"reasoning":"private chain of thought"})),
			delta(json!({"content":"Hello, "})),
			delta(json!({"content":"世界"})),
			finish("stop"),
			usage(),
		],
		true,
	);

	// Act
	let (response, progress) = assemble(&body, split, 1_048_576);

	// Assert
	same_outcome(
		response,
		json!({"choices":[{"finish_reason":"stop","message":{"content":"Hello, 世界"}}],
			"usage":{"prompt_tokens":12,"completion_tokens":7}}),
	);
	assert_eq!(
		progress,
		[
			InferenceProgress::Text {
				text: "Hello, ".into()
			},
			InferenceProgress::Text {
				text: "世界".into()
			},
		]
	);
}

#[rstest::rstest]
fn streamed_tool_calls_match_and_never_disclose_arguments(#[values(1, 13, 4096)] split: usize) {
	// Arrange
	let body = sse(
		&[
			delta(
				json!({"tool_calls":[{"index":0,"id":"call-1","type":"function",
				"function":{"name":"read","arguments":""}}]}),
			),
			delta(json!({"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]})),
			delta(
				json!({"tool_calls":[{"index":1,"id":"call-2","type":"function",
				"function":{"name":"write","arguments":"{\"secret\":"}}]}),
			),
			delta(json!({"tool_calls":[{"index":0,"function":{"arguments":"\"notes\"}"}}]})),
			delta(json!({"tool_calls":[{"index":1,"function":{"arguments":"\"hidden\"}"}}]})),
			finish("tool_calls"),
			usage(),
		],
		true,
	);

	// Act
	let (response, progress) = assemble(&body, split, 1_048_576);

	// Assert
	same_outcome(
		response,
		json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[
			{"id":"call-1","type":"function","function":{"name":"read","arguments":"{\"path\":\"notes\"}"}},
			{"id":"call-2","type":"function","function":{"name":"write","arguments":"{\"secret\":\"hidden\"}"}}
		]}}],"usage":{"prompt_tokens":12,"completion_tokens":7}}),
	);
	let disclosed = serde_json::to_string(&progress).unwrap();
	for argument in ["path", "notes", "secret", "hidden"] {
		assert!(!disclosed.contains(argument), "disclosed {argument}");
	}
	assert_eq!(
		progress.last().unwrap(),
		&InferenceProgress::ToolCall {
			index: 1,
			id: Some("call-2".into()),
			name: Some("write".into()),
			argument_bytes: "{\"secret\":\"hidden\"}".len() as u64,
		}
	);
	assert!(progress.contains(&InferenceProgress::ToolCall {
		index: 0,
		id: Some("call-1".into()),
		name: Some("read".into()),
		argument_bytes: "{\"path\":".len() as u64,
	}));
}

#[rstest::rstest]
#[case::missing_done(vec![
	delta(json!({"tool_calls":[{"index":0,"id":"call-1","function":{"name":"run","arguments":"{}"}}]})),
	finish("tool_calls"),
], false)]
#[case::missing_finish_reason(vec![
	delta(json!({"tool_calls":[{"index":0,"id":"call-1","function":{"name":"run","arguments":"{}"}}]})),
], true)]
fn incomplete_streams_produce_no_response(#[case] chunks: Vec<Value>, #[case] done: bool) {
	// Act
	let (response, _) = assemble(&sse(&chunks, done), 4096, 1_048_576);

	// Assert
	assert!(matches!(response, Err(Error::External(_))));
}

#[rstest::rstest]
#[case::length(
	vec![delta(json!({"content":"partial"})), finish("length")],
	json!({"finish_reason":"length","message":{"content":"partial"}})
)]
#[case::refusal(
	vec![delta(json!({"refusal":"I cannot"})), finish("stop")],
	json!({"finish_reason":"stop","message":{"content":"","refusal":"I cannot"}})
)]
#[case::duplicate_ids(
	vec![
		delta(json!({"tool_calls":[{"index":0,"id":"same","function":{"name":"a","arguments":"{}"}}]})),
		delta(json!({"tool_calls":[{"index":1,"id":"same","function":{"name":"b","arguments":"{}"}}]})),
		finish("tool_calls"),
	],
	json!({"finish_reason":"tool_calls","message":{"tool_calls":[
		{"id":"same","function":{"name":"a","arguments":"{}"}},
		{"id":"same","function":{"name":"b","arguments":"{}"}}
	]}})
)]
#[case::empty_id(
	vec![
		delta(json!({"tool_calls":[{"index":0,"id":"","function":{"name":"a","arguments":"{}"}}]})),
		finish("tool_calls"),
	],
	json!({"finish_reason":"tool_calls","message":{"tool_calls":[
		{"function":{"name":"a","arguments":"{}"}}
	]}})
)]
#[case::finish_reason_disagrees_with_calls(
	vec![
		delta(json!({"tool_calls":[{"index":0,"id":"one","function":{"name":"a","arguments":"{}"}}]})),
		finish("stop"),
	],
	json!({"finish_reason":"stop","message":{"tool_calls":[
		{"id":"one","function":{"name":"a","arguments":"{}"}}
	]}})
)]
#[case::arguments_are_not_an_object(
	vec![
		delta(json!({"tool_calls":[{"index":0,"id":"one","function":{"name":"a","arguments":"[1]"}}]})),
		finish("tool_calls"),
	],
	json!({"finish_reason":"tool_calls","message":{"tool_calls":[
		{"id":"one","function":{"name":"a","arguments":"[1]"}}
	]}})
)]
#[case::arguments_are_not_json(
	vec![
		delta(json!({"tool_calls":[{"index":0,"id":"one","function":{"name":"a","arguments":"{\"a\":"}}]})),
		finish("tool_calls"),
	],
	json!({"finish_reason":"tool_calls","message":{"tool_calls":[
		{"id":"one","function":{"name":"a","arguments":"{\"a\":"}}
	]}})
)]
fn invalid_streams_fail_like_non_streamed_responses(
	#[case] chunks: Vec<Value>,
	#[case] choice: Value,
) {
	// Act
	let (response, _) = assemble(&sse(&chunks, true), 4096, 1_048_576);

	// Assert
	assert!(response.is_err());
	same_outcome(response, json!({"choices":[choice]}));
}

#[rstest::rstest]
fn mid_stream_errors_are_sanitized_provider_rejections() {
	// Arrange
	let error = json!({"error":{"code":503,"message":"token=private upstream failure"},
		"choices":[{"index":0,"delta":{"content":""},"finish_reason":"error"}]});
	let unexplained = finish("error");
	let audio = json!({"error":{"code":"invalid","message":"Audio exceeds the duration limit"}});

	// Act
	let results = [error, unexplained, audio].map(|chunk| {
		assemble(
			&sse(&[delta(json!({"content":"Hi"})), chunk], true),
			4096,
			1_048_576,
		)
		.0
	});

	// Assert
	assert!(
		matches!(&results[0], Err(Error::ProviderRejected { status: 503, reason })
		if reason == "upstream rejected the request")
	);
	assert!(
		matches!(&results[1], Err(Error::ProviderRejected { status: 502, reason })
		if reason == "upstream rejected the request")
	);
	assert!(
		matches!(&results[2], Err(Error::ProviderRejected { status: 502, reason })
		if reason == "Audio exceeds the provider limit")
	);
}

#[rstest::rstest]
fn oversized_streams_fail_like_oversized_bodies() {
	// Arrange
	let chunks: Vec<Value> = (0..8)
		.map(|_| delta(json!({"content":"0123456789"})))
		.chain([finish("stop")])
		.collect();
	let unterminated = vec![b'x'; 128];

	// Act
	let (assembled, _) = assemble(&sse(&chunks, true), 4096, 64);
	let (line, _) = assemble(&unterminated, 16, 64);

	// Assert
	for result in [assembled, line] {
		assert!(
			matches!(result, Err(Error::External(message)) if message == "response exceeds 64 bytes")
		);
	}
}

#[rstest::rstest]
#[case::reasoning(json!({"choices":[{"delta":{"reasoning":"x"}}]}))]
#[case::unknown_field(json!({"padding":"0123456789012345678901234567"}))]
fn events_without_assembled_text_count_toward_the_stream_cap(#[case] event: Value) {
	// Arrange: each event fits the 64-byte line bound and adds nothing assembled.
	let events = vec![event; 8192 / 40 + 1];
	let body = sse(&events, true);

	// Act
	let (result, _) = assemble(&body, 4096, 64);

	// Assert
	assert!(
		matches!(result, Err(Error::External(message)) if message == "response stream exceeds 8192 bytes")
	);
}

#[rstest::rstest]
#[case::both_unindexed(json!([{"delta":{"content":"one"}},{"delta":{"content":"two"}}]))]
#[case::one_unindexed(json!([{"index":0,"delta":{"content":"one"}},{"delta":{"content":"two"}}]))]
fn several_unindexed_choices_are_rejected_as_ambiguous(#[case] choices: Value) {
	// Arrange
	let body = sse(&[json!({"choices":choices}), finish("stop")], true);

	// Act
	let (result, _) = assemble(&body, 4096, 1_048_576);

	// Assert
	assert!(
		matches!(result, Err(Error::External(message)) if message == "provider stream returned ambiguous unindexed choices")
	);
}

#[rstest::rstest]
fn a_sole_unindexed_choice_is_choice_zero() {
	// Arrange
	let body = sse(
		&[
			json!({"choices":[{"delta":{"content":"only"}}]}),
			json!({"choices":[{"index":1,"delta":{"content":"other"}}]}),
			finish("stop"),
		],
		true,
	);

	// Act
	let (result, _) = assemble(&body, 4096, 1_048_576);

	// Assert
	assert_eq!(result.unwrap().text, "only");
}

#[rstest::rstest]
fn only_data_events_count_as_liveness() {
	// Arrange
	let (recorder, received) = recorder();
	let mut assembler = StreamAssembler::new(&recorder, 1_048_576);

	// Act
	let comment = assembler.push(b": OPENROUTER PROCESSING\n\n").unwrap();
	let blank = assembler.push(b"\n").unwrap();
	let reasoning = assembler
		.push(format!("data: {}\n\n", delta(json!({"reasoning":"thinking"}))).as_bytes())
		.unwrap();

	// Assert
	assert!(!comment && !blank);
	assert!(reasoning);
	assert!(received.try_recv().is_err());
}

#[rstest::rstest]
fn large_text_deltas_are_offered_within_the_item_bound() {
	// Arrange
	let text = "é".repeat(MAX_PROGRESS_ITEM_BYTES);
	let body = sse(&[delta(json!({"content":text})), finish("stop")], true);

	// Act
	let (response, progress) = assemble(&body, 4096, 1_048_576);

	// Assert
	assert_eq!(response.unwrap().text, text);
	assert!(
		progress
			.iter()
			.all(|item| item.weight() <= MAX_PROGRESS_ITEM_BYTES / 2)
	);
	let offered: String = progress
		.into_iter()
		.map(|item| match item {
			InferenceProgress::Text { text } => text,
			other => panic!("unexpected progress {other:?}"),
		})
		.collect();
	assert_eq!(offered, text);
}
