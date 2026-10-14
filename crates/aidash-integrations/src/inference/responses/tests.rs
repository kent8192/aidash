use super::*;
use aidash_application::provider_access::Access;
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

#[test]
fn ordered_contexts_send_both_parts_in_order() {
	let mut request = request();
	request.context =
		aidash_domain::provider::ModelContext::Ordered(aidash_domain::provider::OrderedContext {
			stable: "stable-part".into(),
			volatile: "volatile-part".into(),
		});
	let body = request_body(&config(), &request, None).unwrap();
	let content = &body["input"][0]["content"];
	assert_eq!(
		content[0],
		json!({"type":"input_text","text":"stable-part"})
	);
	assert_eq!(
		content[1],
		json!({"type":"input_text","text":"volatile-part"})
	);
	assert_eq!(body["instructions"], "Be precise.");
}

#[test]
fn audio_input_is_refused_before_any_request() {
	let mut request = request();
	request.content_parts = vec![ContentPart::Audio {
		format: "wav".into(),
		bytes: vec![0; 4],
	}];
	assert!(matches!(
		request_body(&config(), &request, None),
		Err(Error::Plan(Failure::Unsupported(_)))
	));
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
#[test]
fn responses_beyond_the_requested_output_are_not_acted_on() {
	let stream = sse(&[completed_event(json!([
		{"type":"message","content":[{"type":"output_text","text":"Looking."}]},
		{"type":"function_call","call_id":"call_1","name":"search","arguments":"{}"}
	]))]);
	let (result, _) = assemble(&stream);
	let output = result.as_ref().unwrap().reported.output_tokens.unwrap();
	let capped = u32::try_from(output).unwrap();
	assert!(within_output_cap(assemble(&stream).0, capped).is_ok());
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
		("content_filter", ContextFailure::Refused),
	] {
		let stream = sse(&[json!({"type":"response.incomplete","response":{
			"status":"incomplete","incomplete_details":{"reason":reason},
			"usage":{"input_tokens":10,"output_tokens":5}
		}})]);
		let (result, _) = assemble(&stream);
		match result {
			Err(Error::TerminalResponse(failure, usage)) => {
				assert_eq!(failure, expected);
				assert_eq!((usage.input_tokens, usage.output_tokens), (10, 5));
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
		})
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
			client: reqwest::Client::new(),
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
	let (rejecting, _server) = provider_with_rejecting_server().await;
	assert!(matches!(
		rejecting.0.infer(request(), &Recorder::default()).await,
		Err(Error::Plan(Failure::NeedsReconnection))
	));
}
async fn provider_with_rejecting_server() -> ((ResponsesProvider, Arc<FakeAccess>), Server) {
	let app = Router::new().route(
		"/responses",
		post(|| async { (StatusCode::UNAUTHORIZED, Json(json!({"detail":"no"}))) }),
	);
	let (url, server) = serve(app).await;
	(provider(&url, false), server)
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
