use super::*;
use aidash_application::provider_access::Access;
use aidash_domain::{
	agent_plugin::interaction::SamplingMessage,
	projection::native::{Envelope, NativeContext, NativeMessage, Source as NativeSource, Trust},
	provider::{ModelContext, OrderedContext, SamplingContext},
};
use axum::{
	Json, Router,
	http::{HeaderMap, StatusCode, header},
	response::IntoResponse,
	routing::post,
};
use std::sync::Mutex;

#[derive(Default)]
struct Recorder(Mutex<Vec<InferenceProgress>>);
impl InferenceProgressSink for Recorder {
	fn offer(&self, progress: InferenceProgress) {
		self.0.lock().unwrap().push(progress);
	}
}

fn config() -> ModelConfig {
	serde_json::from_value(json!({
		"provider":"chatgpt","model_id":"gpt-fixture","endpoint":"https://api.openai.com/v1",
		"plan_connection":"chatgpt","context_window":128000,"max_output_tokens":16384,
		"modalities":["text","image"],"cost":{},"reasoning_effort":"high"
	}))
	.unwrap()
}
fn request() -> ModelRequest {
	ModelRequest {
		instructions: "Be precise.".into(),
		context: json!({"task":"summarize"}).into(),
		tools: vec![aidash_domain::provider::ToolSpec {
			name: "search".into(),
			description: "Search the workspace".into(),
			parameters: json!({"type":"object","properties":{"q":{"type":"string"}}}),
		}],
		max_output_tokens: 4096,
		..Default::default()
	}
}
fn sse(events: &[Value]) -> String {
	events
		.iter()
		.map(|event| {
			format!(
				"event: {}\ndata: {event}\n\n",
				event["type"].as_str().unwrap()
			)
		})
		.collect()
}
fn assemble(stream: &str) -> (Result<ModelResponse>, Vec<InferenceProgress>) {
	let recorder = Recorder::default();
	let mut assembler = Assembler::new(&recorder, MAX_RESPONSE_BYTES);
	// Deliver in small chunks to exercise line framing across chunk edges.
	for chunk in stream.as_bytes().chunks(7) {
		if assembler.done() {
			break;
		}
		if let Err(error) = assembler.push(chunk) {
			return (Err(error), recorder.0.into_inner().unwrap());
		}
	}
	let result = assembler.finish();
	(result, recorder.0.into_inner().unwrap())
}
fn completed_event(output: Value) -> Value {
	json!({"type":"response.completed","response":{
		"status":"completed","output":output,
		"usage":{"input_tokens":1200,"input_tokens_details":{"cached_tokens":800},"output_tokens":90,"output_tokens_details":{"reasoning_tokens":40}}
	}})
}

#[test]
fn plan_keeps_its_request_bytes_and_ignores_openrouter_only_events_and_usage() {
	let body = request_body(&config(), &request(), Some("salt-line\n")).unwrap();
	assert_eq!(
		body.to_string(),
		r#"{"input":[{"content":[{"text":"{\"task\":\"summarize\"}","type":"input_text"}],"role":"user"}],"instructions":"salt-line\nBe precise.","model":"gpt-fixture","reasoning":{"effort":"high"},"store":false,"stream":true,"tools":[{"description":"Tools of this Aidash agent.","name":"aidash","tools":[{"description":"Search the workspace","name":"search","parameters":{"properties":{"q":{"type":"string"}},"type":"object"},"type":"function"}],"type":"namespace"}]}"#
	);
	let mut terminal = completed_event(
		json!([{"type":"message","content":[{"type":"output_text","text":"complete"}]}]),
	);
	terminal["response"]["usage"]["input_tokens_details"]["cache_write_tokens"] = json!(10);
	terminal["response"]["usage"]["cost"] = json!(0.1);
	let mut router_done = terminal.clone();
	router_done["type"] = json!("response.done");
	router_done["response"]["output"][0]["content"][0]["text"] = json!("early");
	let (result, progress) = assemble(&sse(&[
		json!({"type":"response.content_part.delta","delta":"early"}),
		router_done,
		terminal,
	]));
	let result = result.unwrap();
	assert_eq!(result.text, "complete");
	assert_eq!(result.reported.cache_write_tokens, None);
	assert_eq!(result.reported.cost, None);
	assert!(progress.is_empty());
}

#[test]
fn requests_carry_the_projected_context_without_server_state() {
	let mut request = request();
	request.content_parts = vec![ContentPart::Image {
		media_type: "image/png".into(),
		bytes: vec![1, 2, 3],
	}];
	request.response_format =
		Some(aidash_domain::context::summary::SummaryContent::response_format());
	let body = request_body(&config(), &request, Some("salt-line\n")).unwrap();
	assert_eq!(body["model"], "gpt-fixture");
	assert_eq!(body["instructions"], "salt-line\nBe precise.");
	// Plan usage requires both, on every request.
	assert_eq!(body["store"], false);
	assert_eq!(body["stream"], true);
	let content = body["input"][0]["content"].as_array().unwrap();
	assert_eq!(body["input"][0]["role"], "user");
	assert_eq!(content[0]["type"], "input_text");
	assert_eq!(content[0]["text"], json!({"task":"summarize"}).to_string());
	assert_eq!(content[1]["type"], "input_image");
	assert!(
		content[1]["image_url"]
			.as_str()
			.unwrap()
			.starts_with("data:image/png;base64,")
	);
	// Plan usage accepts function tools only inside a namespace.
	assert_eq!(
		body["tools"],
		json!([{"type":"namespace","name":"aidash","description":"Tools of this Aidash agent.","tools":[{"type":"function","name":"search","description":"Search the workspace","parameters":{"type":"object","properties":{"q":{"type":"string"}}}}]}])
	);
	assert_eq!(body["text"]["format"]["type"], "json_schema");
	assert_eq!(body["text"]["format"]["name"], "execution_summary");
	assert_eq!(body["text"]["format"]["strict"], true);
	assert!(body["text"]["format"]["schema"].is_object());
	assert_eq!(body["reasoning"], json!({"effort":"high"}));
	// Parameters plan usage ignores are never sent; the limit is local.
	for absent in [
		"max_output_tokens",
		"temperature",
		"top_p",
		"previous_response_id",
		"provider",
		"messages",
	] {
		assert!(body.get(absent).is_none(), "{absent}");
	}
}

#[rstest::rstest]
#[case::ordered(
	ModelContext::Ordered(OrderedContext { stable: "stable-part".into(), volatile: "volatile-part".into() }),
	json!([{"role":"user","content":[{"type":"input_text","text":"stable-part"},{"type":"input_text","text":"volatile-part"}]}])
)]
#[case::sampling(
	ModelContext::Sampling(SamplingContext { sampling_messages: vec![
		SamplingMessage { role: "assistant".into(), text: "Earlier answer".into() },
		SamplingMessage { role: "user".into(), text: "Server follow-up".into() },
		SamplingMessage { role: "assistant".into(), text: "Server supplied draft".into() },
	] }),
	json!([{"role":"assistant","content":[{"type":"input_text","text":"Earlier answer"}]},{"role":"user","content":[{"type":"input_text","text":"Server follow-up"}]},{"role":"assistant","content":[{"type":"input_text","text":"Server supplied draft"}]}])
)]
fn request_contexts_preserve_all_content_and_message_roles(
	#[case] context: ModelContext,
	#[case] expected: Value,
) {
	let mut request = request();
	request.context = context;
	request.tools.clear();
	let body = request_body(&config(), &request, None).unwrap();
	assert_eq!(body["input"], expected);
	assert_eq!(body["instructions"], "Be precise.");
}

fn envelope(source: NativeSource, content: Value) -> Envelope {
	Envelope {
		source,
		provenance: json!({"journal":"context"}),
		revision: Some(7),
		trust: Trust::Data,
		content,
	}
}

fn native_request() -> ModelRequest {
	ModelRequest {
		context: ModelContext::Native(NativeContext {
			attachments: vec![envelope(NativeSource::Task, json!("original task"))],
			history: vec![],
			volatile: json!({"step":1}).to_string(),
		}),
		..request()
	}
}

#[test]
fn native_requests_preserve_all_messages_calls_results_and_media_in_order() {
	let mut request = native_request();
	let ModelContext::Native(native) = &mut request.context else {
		unreachable!()
	};
	native.history = vec![
		NativeMessage::Assistant {
			text: Some("Looking up both sources.".into()),
			tool_calls: vec![
				ToolCall {
					id: "call-a".into(),
					name: "search".into(),
					arguments: json!({"q":"first"}),
				},
				ToolCall {
					id: "call-b".into(),
					name: "search".into(),
					arguments: json!({"q":"second"}),
				},
			],
		},
		NativeMessage::Tool {
			tool_call_id: "call-a".into(),
			envelope: envelope(NativeSource::Tool, json!({"found":"first"})),
		},
		NativeMessage::Tool {
			tool_call_id: "call-b".into(),
			envelope: envelope(NativeSource::Tool, json!({"found":"second"})),
		},
		NativeMessage::Human {
			request: envelope(NativeSource::HumanRequest, json!("Proceed?")),
			response: json!("yes"),
		},
		NativeMessage::Assistant {
			text: Some("Continuing after approval.".into()),
			tool_calls: vec![],
		},
		NativeMessage::Data {
			envelope: envelope(NativeSource::ModelMediaObservation, json!("observed image")),
		},
	];
	request.content_parts = vec![ContentPart::Image {
		media_type: "image/png".into(),
		bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
	}];
	request.validate().unwrap();
	let body = request_body(&config(), &request, Some("salt-line\n")).unwrap();
	assert_eq!(body["instructions"], "salt-line\nBe precise.");
	assert_eq!(body["store"], false);
	assert_eq!(body["stream"], true);
	let input = body["input"].as_array().unwrap();
	assert_eq!(input.len(), 10);
	let data = |index: usize| {
		serde_json::from_str::<Value>(input[index]["content"][0]["text"].as_str().unwrap()).unwrap()
	};
	assert_eq!(input[0]["role"], "user");
	assert_eq!(data(0)["content"], "original task");
	assert_eq!(data(0)["trust"], "data");
	assert_eq!(
		input[1],
		json!({"role":"assistant","content":[{"type":"input_text","text":"Looking up both sources."}]})
	);
	for (index, call_id, arguments) in [
		(2, "call-a", r#"{"q":"first"}"#),
		(3, "call-b", r#"{"q":"second"}"#),
	] {
		assert_eq!(
			input[index],
			json!({"type":"function_call","call_id":call_id,"namespace":"aidash","name":"search","arguments":arguments})
		);
	}
	for (index, call_id, found) in [(4, "call-a", "first"), (5, "call-b", "second")] {
		assert_eq!(input[index]["type"], "function_call_output");
		assert_eq!(input[index]["call_id"], call_id);
		let output: Value = serde_json::from_str(input[index]["output"].as_str().unwrap()).unwrap();
		assert_eq!(output["content"]["found"], found);
		assert_eq!(output["trust"], "data");
	}
	assert_eq!(input[6]["role"], "user");
	assert_eq!(data(6)["response"], "yes");
	assert_eq!(data(6)["in_reply_to"]["source"], "human_request");
	assert_eq!(
		input[7],
		json!({"role":"assistant","content":[{"type":"input_text","text":"Continuing after approval."}]})
	);
	assert_eq!(data(8)["source"], "model_media_observation");
	assert_eq!(data(9)["content"], json!({"step":1}));
	assert_eq!(
		input[9]["content"][1],
		json!({"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo="})
	);
	assert!(body.get("previous_response_id").is_none());
	assert!(body.get("include").is_none());
	assert!(input.iter().all(|item| item["type"] != "reasoning"));
}

#[test]
fn audio_input_is_refused_before_any_request() {
	for mut request in [request(), native_request()] {
		request.content_parts = vec![ContentPart::Audio {
			format: "wav".into(),
			bytes: vec![0; 4],
		}];
		assert!(matches!(
			request_body(&config(), &request, None),
			Err(Error::Plan(Failure::Unsupported(
				aidash_domain::plan_connections::Unsupported::Input
			)))
		));
	}
}

#[test]
fn completed_streams_yield_text_tool_calls_and_usage_without_cost() {
	let stream = sse(&[
		json!({"type":"response.created","response":{"status":"in_progress"}}),
		json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_1"}}),
		json!({"type":"response.output_text.delta","output_index":1,"delta":"Look"}),
		json!({"type":"response.output_text.delta","output_index":1,"delta":"ing."}),
		json!({"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","call_id":"call_1","name":"search","arguments":""}}),
		json!({"type":"response.function_call_arguments.delta","output_index":2,"delta":"{\"q\":"}),
		json!({"type":"response.function_call_arguments.delta","output_index":2,"delta":"\"Rust\"}"}),
		completed_event(json!([
			{"type":"reasoning","id":"rs_1","summary":[]},
			{"type":"message","content":[{"type":"output_text","text":"Looking."}]},
			{"type":"function_call","call_id":"call_1","name":"search","arguments":"{\"q\":\"Rust\"}"}
		])),
	]);
	let (result, progress) = assemble(&stream);
	let result = result.unwrap();
	assert_eq!(result.text, "Looking.");
	assert_eq!(result.tool_calls.len(), 1);
	assert_eq!(result.tool_calls[0].id, "call_1");
	assert_eq!(result.tool_calls[0].arguments, json!({"q":"Rust"}));
	assert_eq!((result.input_tokens, result.output_tokens), (1200, 90));
	assert!(result.usage_complete);
	assert_eq!(result.reported.cache_read_tokens, Some(800));
	assert_eq!(result.reported.reasoning_tokens, Some(40));
	assert!(result.reported.cost.is_none());
	assert!(result.reported.cache_write_tokens.is_none());
	// Progress carries text and call identity and size, never argument text.
	assert!(progress.contains(&InferenceProgress::Text {
		text: "Look".into()
	}));
	assert!(progress.contains(&InferenceProgress::ToolCall {
		index: 2,
		id: Some("call_1".into()),
		name: Some("search".into()),
		argument_bytes: 12,
	}));
	assert!(!format!("{progress:?}").contains("Rust"));
}

#[test]
fn usage_limits_after_the_stream_opened_fail_the_run() {
	let stream = sse(&[
		json!({"type":"response.output_text.delta","output_index":0,"delta":"partial"}),
		json!({"type":"response.failed","response":{"status":"failed","error":{"code":"subscription_sharing_usage_limit_exceeded","message":"limit canary"}}}),
	]);
	let (result, _) = assemble(&stream);
	let error = result.unwrap_err();
	assert!(matches!(error, Error::Plan(Failure::UsageLimit)));
	assert!(!error.to_string().contains("canary"));
}

/// Calls into the Aidash namespace map back to the Aidash tool names.
#[test]
fn namespaced_tool_calls_keep_the_aidash_tool_name() {
	let stream = sse(&[
		json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","namespace":"aidash","name":"search","arguments":""}}),
		completed_event(json!([
			{"type":"function_call","call_id":"call_1","namespace":"aidash","name":"search","arguments":"{}"},
			{"type":"function_call","call_id":"call_2","name":"aidash.search","arguments":"{}"}
		])),
	]);
	let (result, _) = assemble(&stream);
	let names = result
		.unwrap()
		.tool_calls
		.into_iter()
		.map(|call| call.name)
		.collect::<Vec<_>>();
	assert_eq!(names, ["search", "search"]);
}

/// An overflow reported after the stream opened enters compact-and-retry
/// rather than a transient retry of the same request.
#[rstest::rstest]
#[case::failed_event(json!({"type":"response.failed","response":{"status":"failed","error":{"code":"context_length_exceeded","message":"too long"}}}))]
#[case::error_event(json!({"type":"error","code":"invalid_request_error","message":"This model's maximum context length is 128000 tokens"}))]
fn streamed_context_overflow_keeps_its_classification(#[case] event: Value) {
	let (result, _) = assemble(&sse(&[event]));
	assert!(matches!(result, Err(Error::ContextOverflow)), "{result:?}");
}

/// Plan usage ignores the request's output limit; a response that reports
/// more output than requested is never acted on, but its usage still settles.
#[rstest::rstest]
#[case::completed(false)]
#[case::incomplete(true)]
fn responses_beyond_the_requested_output_are_not_acted_on(#[case] incomplete: bool) {
	let mut event = completed_event(json!([
		{"type":"message","content":[{"type":"output_text","text":"Looking."}]},
		{"type":"function_call","call_id":"call_1","name":"search","arguments":"{}"}
	]));
	if incomplete {
		event["type"] = json!("response.incomplete");
		event["response"]["status"] = json!("incomplete");
		event["response"]["incomplete_details"] = json!({"reason":"max_output_tokens"});
	}
	let stream = sse(&[event]);
	let (result, _) = assemble(&stream);
	let response = result
		.as_ref()
		.unwrap_or_else(|error| error.terminal_usage().expect("a truncated completion"));
	let output = response.reported.output_tokens.unwrap();
	let capped = u32::try_from(output).unwrap();
	let within = within_output_cap(assemble(&stream).0, capped);
	assert_eq!(within.is_err(), incomplete);
	let response = within
		.as_ref()
		.unwrap_or_else(|error| error.terminal_usage().expect("a truncated completion"));
	assert_eq!(response.text, "Looking.");
	match within_output_cap(result, capped - 1) {
		Err(Error::TerminalResponse(ContextFailure::OutputTruncated, usage)) => {
			assert!(usage.tool_calls.is_empty() && usage.text.is_empty());
			assert_eq!(usage.reported.output_tokens, Some(output));
		}
		other => panic!("{other:?}"),
	}
}

#[test]
fn unavailable_plan_usage_after_the_stream_opened_is_transient() {
	let stream = sse(&[
		json!({"type":"error","code":"subscription_sharing_usage_unavailable","message":"later"}),
	]);
	let (result, _) = assemble(&stream);
	assert!(matches!(result, Err(Error::External(_))));
}

#[test]
fn truncated_and_filtered_responses_keep_their_usage() {
	for (reason, expected) in [
		("max_output_tokens", ContextFailure::OutputTruncated),
		("max_messages", ContextFailure::OutputTruncated),
		("content_filter", ContextFailure::Refused),
	] {
		let stream = sse(&[json!({"type":"response.incomplete","response":{
			"status":"incomplete","incomplete_details":{"reason":reason},
			"output":[{"type":"message","content":[{"type":"output_text","text":"Partial answer"}]},{"type":"function_call","call_id":"unfinished","name":"write","arguments":"{"}],
			"usage":{"input_tokens":10,"output_tokens":5}
		}})]);
		let (result, _) = assemble(&stream);
		match result {
			Err(Error::TerminalResponse(failure, usage)) => {
				assert_eq!(failure, expected);
				assert_eq!((usage.input_tokens, usage.output_tokens), (10, 5));
				assert_eq!(
					usage.text,
					if expected == ContextFailure::OutputTruncated {
						"Partial answer"
					} else {
						""
					}
				);
				assert!(usage.tool_calls.is_empty());
			}
			other => panic!("{reason}: {other:?}"),
		}
	}
}

#[test]
fn refusals_are_typed_and_never_yield_tool_calls() {
	let result = completed(
		&completed_event(json!([
			{"type":"message","content":[{"type":"refusal","refusal":"no"}]},
			{"type":"function_call","call_id":"call_1","name":"search","arguments":"{}"}
		]))["response"],
	);
	assert!(matches!(
		result,
		Err(Error::TerminalResponse(ContextFailure::Refused, _))
	));
}

#[test]
fn invalid_outputs_are_rejected_like_chat_completions() {
	for output in [
		json!([]),
		json!([{"type":"message","content":[{"type":"output_text","text":"  \n"}]}]),
		json!([
			{"type":"function_call","call_id":"a","name":"x","arguments":"{}"},
			{"type":"function_call","call_id":"a","name":"y","arguments":"{}"}
		]),
		json!([{"type":"function_call","call_id":"a","name":"x","arguments":"[1]"}]),
		json!([{"type":"function_call","call_id":"a","name":"x","arguments":"{"}]),
		json!([{"type":"function_call","name":"x","arguments":"{}"}]),
	] {
		assert!(
			completed(&completed_event(output.clone())["response"]).is_err(),
			"{output}"
		);
	}
}

#[test]
fn streams_without_a_terminal_event_are_incomplete() {
	let stream = sse(&[json!({"type":"response.output_text.delta","output_index":0,"delta":"x"})]);
	let (result, _) = assemble(&stream);
	assert!(matches!(result, Err(Error::External(_))));
}

#[test]
fn oversized_streams_are_refused() {
	let recorder = Recorder::default();
	let mut assembler = Assembler::new(&recorder, 64);
	let event = json!({"type":"response.output_text.delta","delta":"x".repeat(80)});
	assert!(
		assembler
			.push(format!("data: {event}\n\n").as_bytes())
			.is_err()
	);
}

/// Function calls without an identity still cost their structure, so distinct
/// output indices cannot grow the call table without reaching the cap.
#[test]
fn anonymous_function_calls_count_against_the_output_limit() {
	let recorder = Recorder::default();
	let mut assembler = Assembler::new(&recorder, 1024);
	let refused = (0..64).any(|index| {
		let event = json!({"type":"response.output_item.added","output_index":index,
			"item":{"type":"function_call"}});
		assembler
			.push(format!("data: {event}\n\n").as_bytes())
			.is_err()
	});
	assert!(refused);
}

/// Bytes of a data line still arriving keep the stream alive; comments and
/// other fields do not.
#[test]
fn partial_data_lines_are_liveness() {
	let recorder = Recorder::default();
	let mut assembler = Assembler::new(&recorder, MAX_RESPONSE_BYTES);
	assert!(!assembler.push(b": keep-alive\n").unwrap());
	assert!(!assembler.push(b"event: response.completed\n").unwrap());
	assert!(assembler.push(b"data: {\"type\":\"response.").unwrap());
	assert!(assembler.push(b"completed\"").unwrap());
	let mut empty = Assembler::new(&recorder, MAX_RESPONSE_BYTES);
	assert!(!empty.push(b"data:\n").unwrap());
}

#[test]
fn rejections_follow_the_plan_error_table() {
	let cases = [
		(
			429,
			json!({"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"x"}}),
			"usage",
		),
		(401, json!({"detail":"not accepted"}), "refresh"),
		(
			403,
			json!({"error":{"code":"subscription_sharing_user_not_eligible"}}),
			"ineligible",
		),
		(
			503,
			json!({"detail":"direct routing unavailable"}),
			"transient",
		),
		(
			400,
			json!({"error":{"code":"context_length_exceeded","message":"too long"}}),
			"overflow",
		),
		(
			400,
			json!({"error":{"code":"subscription_sharing_unsupported_capability","param":"tools[0]"}}),
			"unsupported",
		),
	];
	for (status, body, expected) in cases {
		let rejection = plan_rejection(status, Some(&body));
		let matched = match expected {
			"usage" => matches!(rejection.error, Error::Plan(Failure::UsageLimit)),
			"refresh" => {
				rejection.disposition == Some(Disposition::RefreshThenReconnect)
					&& matches!(rejection.error, Error::Plan(Failure::NeedsReconnection))
			}
			"ineligible" => rejection.disposition == Some(Disposition::MarkIneligible),
			"transient" => matches!(rejection.error, Error::External(_)),
			"overflow" => matches!(rejection.error, Error::ContextOverflow),
			"unsupported" => matches!(
				rejection.error,
				Error::Plan(Failure::Unsupported(
					aidash_domain::plan_connections::Unsupported::Tools
				))
			),
			_ => unreachable!(),
		};
		assert!(matched, "{status} {body}: {:?}", rejection.error);
	}
}

/// Hands out a bearer that changes after each forced refresh.
struct FakeAccess {
	endpoint: String,
	generation: Mutex<u32>,
	reports: Mutex<Vec<Disposition>>,
	refresh_succeeds: bool,
}
#[async_trait]
impl ProviderAccess for FakeAccess {
	async fn resolve(&self, context: &Context, endpoint: &str, source: &Source) -> Result<Access> {
		assert_eq!(endpoint, "https://api.openai.com/v1");
		assert!(matches!(source, Source::Plan { provider } if provider == "chatgpt"));
		let inference = context.inference.as_ref().unwrap();
		assert_eq!(inference.model, "gpt-fixture");
		Ok(Access {
			endpoint: self.endpoint.clone(),
			bearer: format!("access-{}", self.generation.lock().unwrap()).into(),
			keyless: None,
		})
	}
	async fn check(&self, _: &Context, _: &str, _: &Source) -> Result<()> {
		unreachable!("inference never preflights a credential")
	}
	async fn plan_rejected(&self, _: &Context, disposition: Disposition) -> Result<bool> {
		self.reports.lock().unwrap().push(disposition);
		if disposition == Disposition::RefreshThenReconnect && self.refresh_succeeds {
			*self.generation.lock().unwrap() += 1;
			return Ok(true);
		}
		Ok(false)
	}
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
async fn serve(app: Router) -> (String, Server) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let url = format!("http://{}", listener.local_addr().unwrap());
	(
		url,
		Server(tokio::spawn(async move {
			axum::serve(listener, app).await.unwrap();
		})),
	)
}
fn provider(url: &str, refresh_succeeds: bool) -> (ResponsesProvider, Arc<FakeAccess>) {
	let access = Arc::new(FakeAccess {
		endpoint: url.to_owned(),
		generation: Mutex::new(0),
		reports: Mutex::new(Vec::new()),
		refresh_succeeds,
	});
	(
		ResponsesProvider {
			clients: crate::keyless::Clients::new(
				reqwest::Client::new(),
				std::time::Duration::from_secs(30),
				std::time::Duration::from_secs(5),
			)
			.unwrap(),
			config: config(),
			access: access.clone(),
			context: Context {
				tenant: "brave-otter-k3m9q2xa".into(),
				run: Some(uuid::Uuid::now_v7()),
				..Context::default()
			},
			cache_salt: None,
		},
		access,
	)
}
fn stream_response(events: &[Value]) -> axum::response::Response {
	([(header::CONTENT_TYPE, "text/event-stream")], sse(events)).into_response()
}

#[tokio::test]
async fn plan_calls_stream_from_the_responses_endpoint_with_the_plan_bearer() {
	let seen = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
	let history = seen.clone();
	let app = Router::new().route(
		"/responses",
		post(move |headers: HeaderMap, Json(body): Json<Value>| {
			let history = history.clone();
			async move {
				let bearer = headers[header::AUTHORIZATION].to_str().unwrap().to_owned();
				history.lock().unwrap().push((bearer, body));
				stream_response(&[completed_event(json!([
					{"type":"message","content":[{"type":"output_text","text":"done"}]}
				]))])
			}
		}),
	);
	let (url, _server) = serve(app).await;
	let (plan, access) = provider(&url, true);
	let recorder = Recorder::default();
	let result = plan.infer(request(), &recorder).await.unwrap();
	assert_eq!(result.text, "done");
	let seen = seen.lock().unwrap();
	assert_eq!(seen[0].0, "Bearer access-0");
	assert_eq!(seen[0].1["store"], false);
	assert_eq!(seen[0].1["stream"], true);
	assert!(access.reports.lock().unwrap().is_empty());
}

/// The provider's streamed calls survive a complete tool step and a persisted
/// request reload, including an assistant turn containing calls without text.
/// Reasoning can contribute usage, but never enters the replay or progress.
#[tokio::test]
async fn native_streamed_tool_steps_resume_with_the_complete_observable_history() {
	let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
	let history = seen.clone();
	let app = Router::new().route(
		"/responses",
		post(move |Json(body): Json<Value>| {
			let history = history.clone();
			async move {
				let mut seen = history.lock().unwrap();
				seen.push(body);
				let output = match seen.len() {
					1 => json!([
						{"type":"reasoning","encrypted_content":"opaque-reasoning-never-retained","summary":[{"text":"hidden-summary"}]},
						{"type":"message","content":[{"type":"output_text","text":"Checking two sources."}]},
						{"type":"function_call","namespace":"aidash","call_id":"step-1-a","name":"search","arguments":"{\"q\":\"one\"}"},
						{"type":"function_call","namespace":"aidash","call_id":"step-1-b","name":"search","arguments":"{\"q\":\"two\"}"}
					]),
					2 => json!([
						{"type":"function_call","namespace":"aidash","call_id":"step-2-a","name":"search","arguments":"{\"q\":\"three\"}"}
					]),
					_ => json!([
						{"type":"message","content":[{"type":"output_text","text":"Finished all three."}]}
					]),
				};
				let mut events = vec![json!({"type":"response.reasoning_summary_text.delta","delta":"hidden-summary"})];
				for (index, item) in output.as_array().unwrap().iter().enumerate() {
					match item["type"].as_str() {
						Some("message") => events.push(json!({"type":"response.output_text.delta","output_index":index,"delta":item["content"][0]["text"]})),
						Some("function_call") => {
							let mut added = item.clone();
							added["arguments"] = json!("");
							events.push(json!({"type":"response.output_item.added","output_index":index,"item":added}));
							events.push(json!({"type":"response.function_call_arguments.delta","output_index":index,"delta":item["arguments"]}));
						}
						_ => {},
					}
				}
				events.push(completed_event(output));
				stream_response(&events)
			}
		}),
	);
	let (url, _server) = serve(app).await;
	let (mut plan, _) = provider(&url, true);
	plan.config.projection_versions = vec![aidash_domain::projection::ProjectionVersion::Native];
	let recorder = Recorder::default();
	let mut request = native_request();
	for (step, expected_ids) in [(1, vec!["step-1-a", "step-1-b"]), (2, vec!["step-2-a"])] {
		let result = plan.infer(request.clone(), &recorder).await.unwrap();
		assert_eq!(
			result
				.tool_calls
				.iter()
				.map(|call| call.id.as_str())
				.collect::<Vec<_>>(),
			expected_ids
		);
		assert!(result.tool_calls.iter().all(|call| call.name == "search"));
		assert_eq!(result.reported.reasoning_tokens, Some(40));
		let ModelContext::Native(native) = &mut request.context else {
			unreachable!()
		};
		native.history.push(NativeMessage::Assistant {
			text: (!result.text.is_empty()).then_some(result.text),
			tool_calls: result.tool_calls.clone(),
		});
		for call in result.tool_calls {
			native.history.push(NativeMessage::Tool {
				tool_call_id: call.id,
				envelope: envelope(NativeSource::Tool, json!({"result":call.arguments})),
			});
		}
		native.volatile = json!({"step":step+1}).to_string();
		// A resumed Run rehydrates the projected messages rather than a
		// provider response ID or opaque reasoning blob.
		request = serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
	}
	let result = plan.infer(request, &recorder).await.unwrap();
	assert_eq!(result.text, "Finished all three.");
	assert!(result.tool_calls.is_empty());
	let seen = seen.lock().unwrap();
	assert_eq!(seen.len(), 3);
	let first = seen[0]["input"].as_array().unwrap();
	let second = seen[1]["input"].as_array().unwrap();
	let third = seen[2]["input"].as_array().unwrap();
	assert_eq!(first.len(), 2);
	assert_eq!(second.len(), 7);
	assert_eq!(third.len(), 9);
	assert_eq!(first[0], second[0]);
	assert_eq!(second[..6], third[..6]);
	assert_eq!(second[1]["role"], "assistant");
	for (index, kind, id) in [
		(2, "function_call", "step-1-a"),
		(3, "function_call", "step-1-b"),
		(4, "function_call_output", "step-1-a"),
		(5, "function_call_output", "step-1-b"),
		(6, "function_call", "step-2-a"),
		(7, "function_call_output", "step-2-a"),
	] {
		assert_eq!(third[index]["type"], kind);
		assert_eq!(third[index]["call_id"], id);
	}
	for body in seen.iter() {
		assert_eq!(body["store"], false);
		assert_eq!(body["stream"], true);
		assert!(body.get("previous_response_id").is_none());
		assert!(body.get("include").is_none());
		assert!(!body.to_string().contains("opaque-reasoning-never-retained"));
		assert!(!body.to_string().contains("hidden-summary"));
	}
	let progress = recorder.0.lock().unwrap();
	assert!(progress.iter().any(
		|item| matches!(item, InferenceProgress::Text { text } if text == "Checking two sources.")
	));
	assert!(progress.iter().any(|item| matches!(item, InferenceProgress::ToolCall { id:Some(id), argument_bytes, .. } if id == "step-1-b" && *argument_bytes > 0)));
	assert!(!format!("{progress:?}").contains("hidden-summary"));
	assert!(!format!("{progress:?}").contains("opaque-reasoning-never-retained"));
}

#[tokio::test]
async fn rejected_access_tokens_are_refreshed_once_and_retried() {
	let calls = Arc::new(Mutex::new(Vec::<String>::new()));
	let history = calls.clone();
	let app = Router::new().route(
		"/responses",
		post(move |headers: HeaderMap| {
			let history = history.clone();
			async move {
				let bearer = headers[header::AUTHORIZATION].to_str().unwrap().to_owned();
				history.lock().unwrap().push(bearer.clone());
				if bearer == "Bearer access-0" {
					return (StatusCode::UNAUTHORIZED, Json(json!({"detail":"expired"})))
						.into_response();
				}
				stream_response(&[completed_event(json!([
					{"type":"message","content":[{"type":"output_text","text":"ok"}]}
				]))])
			}
		}),
	);
	let (url, _server) = serve(app).await;
	let (refreshed, access) = provider(&url, true);
	let result = refreshed
		.infer(request(), &Recorder::default())
		.await
		.unwrap();
	assert_eq!(result.text, "ok");
	assert_eq!(
		calls.lock().unwrap().as_slice(),
		["Bearer access-0", "Bearer access-1"]
	);
	assert_eq!(
		access.reports.lock().unwrap().as_slice(),
		[Disposition::RefreshThenReconnect]
	);
	// A refresh that cannot help leaves the Run asking for reconnection.
	let (rejecting, _server) = provider_with_rejecting_server(false).await;
	assert!(matches!(
		rejecting.0.infer(request(), &Recorder::default()).await,
		Err(Error::Plan(Failure::NeedsReconnection))
	));
	// A refreshed token that is rejected again is neither a terminal refresh
	// error nor a confirmed revocation: it is retried as unavailability.
	let (refreshed, _server) = provider_with_rejecting_server(true).await;
	assert!(matches!(
		refreshed.0.infer(request(), &Recorder::default()).await,
		Err(Error::External(_))
	));
	assert_eq!(
		refreshed.1.reports.lock().unwrap().as_slice(),
		[Disposition::RefreshThenReconnect]
	);
}
async fn provider_with_rejecting_server(
	refresh_succeeds: bool,
) -> ((ResponsesProvider, Arc<FakeAccess>), Server) {
	let app = Router::new().route(
		"/responses",
		post(|| async { (StatusCode::UNAUTHORIZED, Json(json!({"detail":"no"}))) }),
	);
	let (url, server) = serve(app).await;
	(provider(&url, refresh_succeeds), server)
}

/// An invalid user reported after the stream opened still reaches the access
/// layer, which refreshes or ends the connection; the stream is not replayed.
#[rstest::rstest]
#[case::refreshed(true)]
#[case::terminal(false)]
#[tokio::test]
async fn streamed_invalid_users_report_a_refresh(#[case] refresh_succeeds: bool) {
	let calls = Arc::new(Mutex::new(0));
	let counted = calls.clone();
	let app = Router::new().route(
		"/responses",
		post(move || {
			*counted.lock().unwrap() += 1;
			async {
				stream_response(&[json!({"type":"response.failed","response":{"status":"failed","error":{"code":"subscription_sharing_invalid_user","message":"canary"}}})])
			}
		}),
	);
	let (url, _server) = serve(app).await;
	let (plan, access) = provider(&url, refresh_succeeds);
	let error = plan
		.infer(request(), &Recorder::default())
		.await
		.unwrap_err();
	assert_eq!(
		access.reports.lock().unwrap().as_slice(),
		[Disposition::RefreshThenReconnect]
	);
	assert_eq!(*calls.lock().unwrap(), 1);
	if refresh_succeeds {
		assert!(matches!(error, Error::External(_)), "{error:?}");
	} else {
		assert!(
			matches!(error, Error::Plan(Failure::NeedsReconnection)),
			"{error:?}"
		);
	}
}

#[tokio::test]
async fn usage_limits_fail_without_a_retry_and_ineligible_plans_are_reported() {
	let app = Router::new().route(
		"/responses",
		post(|Json(body): Json<Value>| async move {
			let code = if body["model"] == "gpt-fixture" {
				"subscription_sharing_usage_limit_exceeded"
			} else {
				unreachable!()
			};
			(
				StatusCode::TOO_MANY_REQUESTS,
				Json(json!({"error":{"code":code,"message":"canary"}})),
			)
		}),
	);
	let (url, _server) = serve(app).await;
	let (limited, access) = provider(&url, true);
	let error = limited
		.infer(request(), &Recorder::default())
		.await
		.unwrap_err();
	assert!(matches!(error, Error::Plan(Failure::UsageLimit)));
	assert!(access.reports.lock().unwrap().is_empty());

	let app = Router::new().route(
		"/responses",
		post(|| async {
			(
				StatusCode::FORBIDDEN,
				Json(json!({"error":{"code":"subscription_sharing_user_not_eligible"}})),
			)
		}),
	);
	let (url, _server) = serve(app).await;
	let (ineligible, access) = provider(&url, true);
	assert!(matches!(
		ineligible.infer(request(), &Recorder::default()).await,
		Err(Error::Plan(Failure::Ineligible))
	));
	assert_eq!(
		access.reports.lock().unwrap().as_slice(),
		[Disposition::MarkIneligible]
	);
}

#[tokio::test]
async fn requests_beyond_the_configured_output_limit_are_refused_locally() {
	let (plan, _) = provider("http://127.0.0.1:9", true);
	let mut request = request();
	request.max_output_tokens = 16_385;
	assert!(matches!(
		plan.infer(request, &Recorder::default()).await,
		Err(Error::Invalid(_))
	));
}

mod openrouter {
	use super::*;
	use aidash_application::provider_access::ProviderAccess;
	use aidash_domain::model::{ProviderRouteEvidence, WireApi};
	use axum::routing::get;
	use std::sync::atomic::{AtomicUsize, Ordering};

	/// The Tenant's Provider Credential is the only accepted source.
	struct TenantAccess {
		endpoint: String,
		calls: Arc<AtomicUsize>,
	}
	#[async_trait]
	impl ProviderAccess for TenantAccess {
		async fn resolve(
			&self,
			context: &Context,
			endpoint: &str,
			source: &Source,
		) -> Result<Access> {
			assert_eq!(endpoint, "https://openrouter.ai/api/v1");
			assert!(matches!(source, Source::Tenant { provider } if provider == "openrouter"));
			assert_eq!(context.inference.as_ref().unwrap().model, "vendor/model");
			self.calls.fetch_add(1, Ordering::SeqCst);
			Ok(Access {
				endpoint: self.endpoint.clone(),
				bearer: "tenant-openrouter-key".into(),
				keyless: None,
			})
		}
		async fn check(&self, _: &Context, _: &str, _: &Source) -> Result<()> {
			unreachable!("inference never preflights a credential")
		}
	}

	fn config(_endpoint: &str, streaming: bool) -> ModelConfig {
		serde_json::from_value(json!({
			"provider":"openrouter","model_id":"vendor/model",
			"endpoint":"https://openrouter.ai/api/v1",
			"provider_credential":"openrouter","wire_api":"responses",
			"context_window":100_000,"max_output_tokens":512,
			"modalities":["text","image"],"cost":{},
			"streaming":streaming,"reasoning_effort":"high"
		}))
		.unwrap()
	}

	fn request() -> ModelRequest {
		let mut request = super::request();
		request.context = json!({"task":"summarize"}).into();
		request.max_output_tokens = 512;
		request
	}

	async fn openrouter_provider(
		app: Router,
		config: ModelConfig,
		calls: Arc<AtomicUsize>,
	) -> (Arc<dyn ModelProvider>, Server) {
		let (url, server) = serve(app).await;
		let access = Arc::new(TenantAccess {
			endpoint: format!("{url}/v1"),
			calls,
		});
		let provider = crate::inference::salted_provider(
			crate::keyless::Clients::new(
				reqwest::Client::new(),
				std::time::Duration::from_secs(30),
				std::time::Duration::from_secs(5),
			)
			.unwrap(),
			config,
			access,
			Context {
				tenant: "brave-otter-k3m9q2xa".into(),
				run: Some(uuid::Uuid::now_v7()),
				..Context::default()
			},
			Some(
				CacheSaltKeys::new(
					&[crate::inference::CacheSaltKey {
						version: 1,
						secret: "fixture-key".into(),
					}],
					1,
				)
				.unwrap(),
			),
		)
		.unwrap();
		(provider, server)
	}

	fn responses_body(output: Value) -> Value {
		json!({
			"id":"resp-1","object":"response","status":"completed","model":"vendor/model",
			"provider":"Actual Provider","output":output,
			"usage":{"input_tokens":1200,"input_tokens_details":{"cached_tokens":800,"cache_write_tokens":10},"output_tokens":90,"output_tokens_details":{"reasoning_tokens":40},"cost":0.0001234,"cost_details":{"upstream_inference_cost":0.0001}}
		})
	}

	#[tokio::test]
	async fn streams_from_the_responses_endpoint_with_tenant_access_and_cost() {
		let seen = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
		let history = seen.clone();
		let app = Router::new().route(
			"/v1/responses",
			post(move |headers: HeaderMap, Json(body): Json<Value>| {
				let history = history.clone();
				async move {
					let bearer = headers[header::AUTHORIZATION].to_str().unwrap().to_owned();
					history.lock().unwrap().push((bearer, body));
					let mut event = completed_event(json!([
						{"type":"message","content":[{"type":"output_text","text":"done"}]},
						{"type":"function_call","call_id":"call-1","name":"search","arguments":"{\"q\":\"x\"}"}
					]));
					event["response"]["usage"]["input_tokens_details"]["cache_write_tokens"] =
						json!(10);
					event["response"]["usage"]["cost"] = json!(0.0001234);
					event["response"]["usage"]["cost_details"] =
						json!({"upstream_inference_cost":0.0001});
					stream_response(&[event])
				}
			}),
		);
		let calls = Arc::new(AtomicUsize::new(0));
		let (provider, _server) =
			openrouter_provider(app, config("http://unused", true), calls.clone()).await;
		let mut request = request();
		request.content_parts = vec![ContentPart::Image {
			media_type: "image/png".into(),
			bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
		}];
		// Media routes are absent, so the route check refuses before traffic.
		assert!(matches!(
			provider.infer(request.clone(), &Recorder::default()).await,
			Err(Error::Invalid(_))
		));
		assert_eq!(calls.load(Ordering::SeqCst), 0);
		request.content_parts.clear();
		let result = provider.infer(request, &Recorder::default()).await.unwrap();
		assert_eq!(result.text, "done");
		assert_eq!(result.tool_calls[0].name, "search");
		assert_eq!(result.reported.cache_read_tokens, Some(800));
		assert_eq!(result.reported.cache_write_tokens, Some(10));
		assert_eq!(result.reported.reasoning_tokens, Some(40));
		assert_eq!(
			result.reported.cost,
			Some(ProviderCost {
				nanocredits: 123_400,
				upstream_nanocredits: Some(100_000)
			})
		);
		assert_eq!(calls.load(Ordering::SeqCst), 1);
		let seen = seen.lock().unwrap();
		assert_eq!(seen[0].0, "Bearer tenant-openrouter-key");
		let body = &seen[0].1;
		assert_eq!(body["model"], "vendor/model");
		assert_eq!(body["stream"], true);
		assert_eq!(body["max_output_tokens"], 512);
		assert_eq!(body["reasoning"], json!({"effort":"high"}));
		assert_eq!(
			body["provider"],
			json!({"zdr":true,"require_parameters":true})
		);
		assert_eq!(body["instructions"], "Be precise.");
		assert!(body.get("store").is_none());
		assert!(body.get("previous_response_id").is_none());
		// OpenRouter takes flat function tools and carries no namespace.
		assert_eq!(body["tools"][0]["type"], "function");
		assert_eq!(body["tools"][0]["name"], "search");
		assert!(body["tools"][0].get("tools").is_none());
		assert!(
			body["input"]
				.as_array()
				.unwrap()
				.iter()
				.all(|item| item.get("namespace").is_none())
		);
	}

	#[tokio::test]
	async fn non_streaming_parses_a_whole_response_with_cost() {
		let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
		let history = seen.clone();
		let app = Router::new().route(
			"/v1/responses",
			post(move |Json(body): Json<Value>| {
				let history = history.clone();
				async move {
					history.lock().unwrap().push(body);
					Json(responses_body(json!([
						{"type":"message","content":[{"type":"output_text","text":"whole"}]}
					])))
				}
			}),
		);
		let calls = Arc::new(AtomicUsize::new(0));
		let (provider, _server) =
			openrouter_provider(app, config("http://unused", false), calls).await;
		let result = provider
			.infer(request(), &Recorder::default())
			.await
			.unwrap();
		assert_eq!(result.text, "whole");
		assert_eq!((result.input_tokens, result.output_tokens), (1200, 90));
		assert_eq!(
			result.reported.cost,
			Some(ProviderCost {
				nanocredits: 123_400,
				upstream_nanocredits: Some(100_000)
			})
		);
		let seen = seen.lock().unwrap();
		assert!(seen[0].get("stream").is_none());
		assert_eq!(seen[0]["max_output_tokens"], 512);
	}

	#[tokio::test]
	async fn non_streaming_incomplete_keeps_billing_and_discards_partial_tools() {
		for (reason, expected, text) in [
			(
				"max_output_tokens",
				ContextFailure::OutputTruncated,
				"partial",
			),
			("content_filter", ContextFailure::Refused, ""),
		] {
			let app = Router::new().route(
				"/v1/responses",
				post(move || async move {
					let mut body = responses_body(json!([
						{"type":"message","content":[{"type":"output_text","text":"partial"}]},
						{"type":"function_call","call_id":"unfinished","name":"search","arguments":"{"}
					]));
					body["status"] = json!("incomplete");
					body["incomplete_details"] = json!({"reason":reason});
					Json(body)
				}),
			);
			let (provider, _server) = openrouter_provider(
				app,
				config("http://unused", false),
				Arc::new(AtomicUsize::new(0)),
			)
			.await;
			let error = provider
				.infer(request(), &Recorder::default())
				.await
				.unwrap_err();
			let Error::TerminalResponse(failure, usage) = error else {
				panic!("{error:?}")
			};
			assert_eq!(failure, expected);
			assert_eq!(usage.text, text);
			assert!(usage.tool_calls.is_empty());
			assert_eq!(usage.output_tokens, 90);
			assert_eq!(usage.reported.cost.unwrap().nanocredits, 123_400);
		}
	}

	#[tokio::test]
	async fn stream_rejections_preserve_explicit_status_and_known_rate_limits() {
		for error in [
			json!({"type":"error","error":{"code":429,"message":"rate limited"}}),
			json!({"type":"error","code":429,"message":"rate limited; maximum context length is unrelated"}),
			json!({"type":"error","error":{"code":"rate_limit_exceeded","message":"rate limited"}}),
		] {
			let app = Router::new().route(
				"/v1/responses",
				post(move || {
					let error = error.clone();
					async move { stream_response(&[error]) }
				}),
			);
			let (provider, _server) = openrouter_provider(
				app,
				config("http://unused", true),
				Arc::new(AtomicUsize::new(0)),
			)
			.await;
			let result = provider.infer(request(), &Recorder::default()).await;
			assert!(
				matches!(result, Err(Error::ProviderRejected { status: 429, .. })),
				"{result:?}"
			);
		}
	}

	#[tokio::test]
	async fn native_image_requests_keep_salt_and_verified_pinned_routing() {
		let seen = Arc::new(Mutex::new(Vec::new()));
		let history = seen.clone();
		let app = Router::new()
			.route("/v1/models/vendor/model/endpoints", get(|| async { Json(json!({"data":{"architecture":{"input_modalities":["text","image"]},"endpoints":[{"tag":"vendor/route","context_length":100000}]}})) }))
			.route("/v1/endpoints/zdr", get(|| async { Json(json!({"data":[{"model_id":"vendor/model","tag":"vendor/route"}]})) }))
			.route("/v1/responses", post(move |Json(body): Json<Value>| {
				let history = history.clone();
				async move {
					history.lock().unwrap().push(body);
					// Some providers answer stream requests with a whole JSON response.
					Json(responses_body(json!([{"type":"message","content":[{"type":"output_text","text":"done"}]}])))
				}
			}));
		let mut config = config("http://unused", true);
		let now = chrono::Utc::now();
		config.projection_versions = vec![aidash_domain::projection::ProjectionVersion::Native];
		config.provider_route = Some(ProviderRouteEvidence {
			tag: "vendor/route".into(),
			source: "approval".into(),
			verified_at: now - chrono::Duration::minutes(1),
			expires_at: now + chrono::Duration::hours(1),
		});
		config.media_routes = vec![aidash_domain::model::MediaRouteEvidence {
			tag: "vendor/route".into(),
			formats: vec!["image/png".into()],
			source: "approval".into(),
			verified_at: now - chrono::Duration::minutes(1),
			expires_at: now + chrono::Duration::hours(1),
		}];
		let calls = Arc::new(AtomicUsize::new(0));
		let (provider, _server) = openrouter_provider(app, config, calls.clone()).await;
		let mut request = super::native_request();
		request.input_format = aidash_domain::provider::InputFormat::Responses;
		request.max_output_tokens = 512;
		request.cache_scope = Some(aidash_domain::projection::CacheScope {
			tenant: "tenant-a".into(),
			key_version: 1,
		});
		request.content_parts = vec![ContentPart::Image {
			media_type: "image/png".into(),
			bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
		}];
		request.disable_provider_transforms = true;
		assert_eq!(
			provider
				.infer(request.clone(), &Recorder::default())
				.await
				.unwrap()
				.text,
			"done"
		);
		assert_eq!(calls.load(Ordering::SeqCst), 2);
		{
			let seen = seen.lock().unwrap();
			assert_eq!(
				seen[0]["provider"],
				json!({"zdr":true,"require_parameters":true,"only":["vendor/route"],"allow_fallbacks":false})
			);
			let instructions = seen[0]["instructions"].as_str().unwrap();
			assert!(instructions.starts_with("aidash-cache-scope:v1:"));
			assert!(instructions.ends_with(&format!("\n{}", request.instructions)));
			assert_eq!(seen[0]["truncation"], "disabled");
			assert!(
				seen[0]["input"]
					.as_array()
					.unwrap()
					.iter()
					.flat_map(|item| item["content"].as_array().into_iter().flatten())
					.any(|part| part["type"] == "input_image")
			);
		}
		request.cache_scope.as_mut().unwrap().key_version = 2;
		assert!(matches!(
			provider.infer(request, &Recorder::default()).await,
			Err(Error::Invalid(_))
		));
		assert_eq!(
			calls.load(Ordering::SeqCst),
			2,
			"missing salt key sends no discovery or inference"
		);
	}

	#[tokio::test]
	async fn keepalive_comments_do_not_keep_a_stalled_stream_alive() {
		let app = Router::new().route(
			"/v1/responses",
			post(|| async {
				let stream = async_stream::stream! {
					loop {
						yield Ok::<_, std::io::Error>(": keepalive\n\n");
						tokio::time::sleep(std::time::Duration::from_millis(10)).await;
					}
				};
				(
					[(header::CONTENT_TYPE, "text/event-stream")],
					axum::body::Body::from_stream(stream),
				)
			}),
		);
		let mut config = config("http://unused", true);
		config.stream_stall_timeout_secs = Some(1);
		let (provider, _server) =
			openrouter_provider(app, config, Arc::new(AtomicUsize::new(0))).await;
		let result = tokio::time::timeout(
			std::time::Duration::from_secs(3),
			provider.infer(request(), &Recorder::default()),
		)
		.await
		.unwrap();
		assert!(matches!(result, Err(Error::InferenceStalled)), "{result:?}");
	}

	#[tokio::test]
	async fn a_pinned_route_is_checked_against_zdr_before_any_response_call() {
		let responses = Arc::new(AtomicUsize::new(0));
		let counter = responses.clone();
		let app = Router::new()
			.route(
				"/v1/models/vendor/model/endpoints",
				get(|| async {
					Json(json!({"data":{"architecture":{"input_modalities":["text"]},"endpoints":[{"tag":"vendor/route","context_length":100000}]}}))
				}),
			)
			.route(
				"/v1/endpoints/zdr",
				get(|| async { Json(json!({"data":[]})) }),
			)
			.route(
				"/v1/responses",
				post(move || {
					responses.fetch_add(1, Ordering::SeqCst);
					async { Json(responses_body(json!([]))) }
				}),
			);
		let mut config = config("http://unused", true);
		let now = chrono::Utc::now();
		config.provider_route = Some(ProviderRouteEvidence {
			tag: "vendor/route".into(),
			source: "administrator approval".into(),
			verified_at: now - chrono::Duration::minutes(1),
			expires_at: now + chrono::Duration::hours(1),
		});
		let calls = Arc::new(AtomicUsize::new(0));
		let (provider, _server) = openrouter_provider(app, config, calls).await;
		let result = provider.infer(request(), &Recorder::default()).await;
		assert!(matches!(result, Err(Error::Invalid(_))));
		assert_eq!(counter.load(Ordering::SeqCst), 0);
	}

	#[tokio::test]
	async fn context_overflow_and_provider_rejections_are_mapped_like_chat() {
		let app = Router::new().route(
			"/v1/responses",
			post(|| async {
				(
					StatusCode::BAD_REQUEST,
					Json(json!({"error":{"code":400,"message":"This endpoint's maximum context length is 8192 tokens"}})),
				)
			}),
		);
		let calls = Arc::new(AtomicUsize::new(0));
		let (provider, _server) =
			openrouter_provider(app, config("http://unused", true), calls).await;
		assert!(matches!(
			provider.infer(request(), &Recorder::default()).await,
			Err(Error::ContextOverflow)
		));
	}

	#[tokio::test]
	async fn omitted_or_chat_wire_api_keeps_the_chat_completions_path() {
		for wire_api in [None, Some(WireApi::Chat)] {
			let seen = Arc::new(Mutex::new(Vec::<String>::new()));
			let history = seen.clone();
			let app = Router::new().route(
				"/v1/chat/completions",
				post(move |body: axum::body::Bytes| {
					let history = history.clone();
					async move {
						history
							.lock()
							.unwrap()
							.push(String::from_utf8(body.to_vec()).unwrap());
						Json(json!({
							"choices":[{"finish_reason":"stop","message":{"content":"chat"}}],
							"usage":{"prompt_tokens":1,"completion_tokens":1}
						}))
					}
				}),
			);
			let mut config = config("http://unused", false);
			config.wire_api = wire_api;
			let calls = Arc::new(AtomicUsize::new(0));
			let (provider, _server) = openrouter_provider(app, config, calls).await;
			let result = provider
				.infer(request(), &Recorder::default())
				.await
				.unwrap();
			assert_eq!(result.text, "chat");
			assert_eq!(
				seen.lock().unwrap().as_slice(),
				[
					r#"{"max_tokens":512,"messages":[{"content":"Be precise.","role":"system"},{"content":"{\"task\":\"summarize\"}","role":"user"}],"model":"vendor/model","provider":{"require_parameters":true,"zdr":true},"reasoning":{"effort":"high"},"tools":[{"function":{"description":"Search the workspace","name":"search","parameters":{"properties":{"q":{"type":"string"}},"type":"object"}},"type":"function"}]}"#
				]
			);
		}
	}

	#[tokio::test]
	async fn openrouter_responses_requires_a_provider_credential() {
		let mut config = config("http://unused", true);
		config.provider_credential = None;
		assert!(config.validate_wire_api().is_err());
		let result = crate::inference::salted_provider(
			crate::keyless::Clients::new(
				reqwest::Client::new(),
				std::time::Duration::from_secs(5),
				std::time::Duration::from_secs(5),
			)
			.unwrap(),
			config,
			Arc::new(TenantAccess {
				endpoint: "http://127.0.0.1:9".into(),
				calls: Arc::new(AtomicUsize::new(0)),
			}),
			Context::default(),
			None,
		);
		assert!(result.is_err(), "{:?}", result.err());
	}
}
