use super::*;
use aidash_domain::{agent_plugin::Header, keyless::PermittedOrigins};
use axum::{
	Json, Router,
	extract::State,
	http::{HeaderMap, StatusCode},
	response::{IntoResponse, Response},
	routing::post,
};
use serde_json::json;
use std::{
	collections::HashSet,
	sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

#[derive(Clone, Copy, Default)]
enum Mode {
	#[default]
	Normal,
	/// Every page lists one tool and offers another page.
	Endless { schema_bytes: usize },
	/// `initialize` answers with a JSON-RPC error.
	RejectInitialize,
	/// `initialize` answers with HTTP 500.
	BreakInitialize,
	/// `tools/list` answers with something that is not MCP.
	GarbageList,
	/// `tools/call` fails after the server received it.
	BreakCall,
}

struct Seen {
	method: String,
	session: Option<String>,
	headers: HeaderMap,
	params: Value,
}

/// A stateful streamable-HTTP MCP server: sessions are issued by `initialize`, answered with 404
/// once forgotten, and ended by `DELETE`.
#[derive(Default)]
struct Fake {
	live: Mutex<HashSet<String>>,
	issued: AtomicUsize,
	lost_calls: AtomicUsize,
	seen: Mutex<Vec<Seen>>,
	deleted: Mutex<Vec<String>>,
	tools: Mutex<Vec<Value>>,
	mode: Mutex<Mode>,
}

fn alpha() -> Value {
	json!({
		"name": "alpha",
		"description": "Alpha tool",
		"inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}}
	})
}

fn beta() -> Value {
	json!({"name": "beta", "inputSchema": {"type": "object"}})
}

fn reply(id: Value, result: Value) -> Response {
	Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

impl Fake {
	fn new(tools: Vec<Value>) -> Arc<Self> {
		Arc::new(Self {
			tools: Mutex::new(tools),
			..Self::default()
		})
	}

	fn set_mode(&self, mode: Mode) {
		*self.mode.lock().unwrap() = mode;
	}

	fn set_tools(&self, tools: Vec<Value>) {
		*self.tools.lock().unwrap() = tools;
	}

	fn forget_sessions(&self) {
		self.live.lock().unwrap().clear();
	}

	fn count(&self, method: &str) -> usize {
		self.seen
			.lock()
			.unwrap()
			.iter()
			.filter(|seen| seen.method == method)
			.count()
	}

	fn requests(&self) -> usize {
		self.seen.lock().unwrap().len()
	}

	/// The session ids that requests other than `initialize` carried.
	fn sessions_used(&self) -> HashSet<Option<String>> {
		self.seen
			.lock()
			.unwrap()
			.iter()
			.filter(|seen| seen.method != "initialize")
			.map(|seen| seen.session.clone())
			.collect()
	}

	fn call_params(&self) -> Vec<Value> {
		self.seen
			.lock()
			.unwrap()
			.iter()
			.filter(|seen| seen.method == "tools/call")
			.map(|seen| seen.params.clone())
			.collect()
	}

	fn deleted(&self) -> Vec<String> {
		self.deleted.lock().unwrap().clone()
	}

	fn page(&self, cursor: &Value) -> Value {
		let mode = *self.mode.lock().unwrap();
		if let Mode::Endless { schema_bytes } = mode {
			let n: usize = cursor.as_str().map_or(0, |cursor| cursor.parse().unwrap());
			return json!({
				"tools": [{
					"name": format!("tool{n}"),
					"inputSchema": {"type": "object", "description": "x".repeat(schema_bytes)}
				}],
				"nextCursor": (n + 1).to_string()
			});
		}
		let tools = self.tools.lock().unwrap().clone();
		if cursor.is_null() {
			json!({"tools": tools[..1], "nextCursor": "second"})
		} else {
			json!({"tools": tools[1..]})
		}
	}

	fn initialize(&self, id: Value, body: &Value) -> Response {
		match *self.mode.lock().unwrap() {
			Mode::RejectInitialize => Json(json!({
				"jsonrpc": "2.0",
				"id": id,
				"error": {"code": -32603, "message": "initialize refused"}
			}))
			.into_response(),
			Mode::BreakInitialize => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
			_ => {
				let session = format!("s{}", self.issued.fetch_add(1, SeqCst) + 1);
				self.live.lock().unwrap().insert(session.clone());
				let result = json!({
					"protocolVersion": body["params"]["protocolVersion"],
					"capabilities": {"tools": {}},
					"serverInfo": {"name": "fake", "version": "1.0.0"}
				});
				([("mcp-session-id", session)], reply(id, result)).into_response()
			}
		}
	}
}

async fn rpc(
	State(fake): State<Arc<Fake>>,
	headers: HeaderMap,
	Json(body): Json<Value>,
) -> Response {
	let method = body["method"].as_str().unwrap_or_default().to_owned();
	let session = headers
		.get("mcp-session-id")
		.and_then(|value| value.to_str().ok())
		.map(str::to_owned);
	fake.seen.lock().unwrap().push(Seen {
		method: method.clone(),
		session: session.clone(),
		headers,
		params: body["params"].clone(),
	});
	let id = body.get("id").cloned();
	if method == "initialize" {
		return fake.initialize(id.unwrap(), &body);
	}
	match session {
		Some(session) if fake.live.lock().unwrap().contains(&session) => {}
		Some(_) => return StatusCode::NOT_FOUND.into_response(),
		None => return StatusCode::BAD_REQUEST.into_response(),
	}
	let Some(id) = id else {
		return StatusCode::ACCEPTED.into_response();
	};
	match method.as_str() {
		"tools/list" if matches!(*fake.mode.lock().unwrap(), Mode::GarbageList) => {
			([("content-type", "text/plain")], "not MCP").into_response()
		}
		"tools/list" => reply(id, fake.page(&body["params"]["cursor"])),
		"tools/call" if matches!(*fake.mode.lock().unwrap(), Mode::BreakCall) => {
			StatusCode::INTERNAL_SERVER_ERROR.into_response()
		}
		"tools/call" => {
			if fake
				.lost_calls
				.fetch_update(SeqCst, SeqCst, |left| left.checked_sub(1))
				.is_ok()
			{
				return StatusCode::NOT_FOUND.into_response();
			}
			let arguments = &body["params"]["arguments"];
			let failed = arguments["fail"] == json!(true);
			let text = if failed {
				"boom".to_owned()
			} else {
				arguments.to_string()
			};
			reply(
				id,
				json!({"content": [{"type": "text", "text": text}], "isError": failed}),
			)
		}
		other => panic!("unexpected MCP method {other:?}"),
	}
}

async fn end_session(State(fake): State<Arc<Fake>>, headers: HeaderMap) -> StatusCode {
	let session = headers["mcp-session-id"].to_str().unwrap().to_owned();
	fake.live.lock().unwrap().remove(&session);
	fake.deleted.lock().unwrap().push(session);
	StatusCode::OK
}

fn router(fake: Arc<Fake>) -> Router {
	Router::new()
		.route(
			"/mcp",
			post(rpc)
				.get(|| async { StatusCode::METHOD_NOT_ALLOWED })
				.delete(end_session),
		)
		.with_state(fake)
}

fn server(name: &str) -> PluginServer {
	PluginServer {
		plugin: PluginName::try_from(name.to_owned()).unwrap(),
		digest: "sha256:copy".into(),
		server: ServerName::try_from("docs".to_owned()).unwrap(),
	}
}

fn remote(origin: &str, headers: &[(&str, &str)]) -> RemoteServer {
	RemoteServer {
		url: format!("{origin}/mcp").parse().unwrap(),
		headers: headers
			.iter()
			.map(|(name, value)| Header {
				name: (*name).to_owned(),
				value: (*value).to_owned(),
			})
			.collect(),
	}
}

async fn bind() -> (tokio::net::TcpListener, String) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let origin = format!("http://{}", listener.local_addr().unwrap());
	(listener, origin)
}

fn sessions(permitted: &[&str], idle: Duration, capacity: usize) -> McpPluginSessions {
	McpPluginSessions::with_limits(
		KeylessAccess::permitting(PermittedOrigins::parse(permitted).unwrap()),
		ToolClients::new().unwrap(),
		idle,
		capacity,
	)
}

/// A fake MCP server at a permitted loopback origin, and the sessions that reach it.
struct Fixture {
	fake: Arc<Fake>,
	config: RemoteServer,
	sessions: McpPluginSessions,
	run: Uuid,
	server: tokio::task::JoinHandle<()>,
}

impl Fixture {
	async fn start() -> Self {
		Self::start_with(&[], IDLE_LIMIT, MAX_SESSIONS).await
	}

	async fn start_with(headers: &[(&str, &str)], idle: Duration, capacity: usize) -> Self {
		let fake = Fake::new(vec![alpha(), beta()]);
		let (listener, origin) = bind().await;
		let app = router(fake.clone());
		Self {
			config: remote(&origin, headers),
			sessions: sessions(&[&origin], idle, capacity),
			fake,
			run: Uuid::new_v4(),
			server: tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
		}
	}

	async fn list(&self) -> Result<Vec<ListedTool>> {
		self.list_for(self.run).await
	}

	async fn list_for(&self, run: Uuid) -> Result<Vec<ListedTool>> {
		self.sessions.list(run, &server("demo"), &self.config).await
	}

	async fn call(&self, tool: &str, digest: &str, arguments: Value) -> Result<Value> {
		self.call_for(self.run, tool, digest, arguments).await
	}

	async fn call_for(
		&self,
		run: Uuid,
		tool: &str,
		digest: &str,
		arguments: Value,
	) -> Result<Value> {
		self.sessions
			.call(
				run,
				&server("demo"),
				&self.config,
				tool,
				digest,
				arguments,
				"key-1",
			)
			.await
	}
}

impl Drop for Fixture {
	fn drop(&mut self) {
		self.server.abort();
	}
}

async fn until(condition: impl Fn() -> bool) {
	for _ in 0..500 {
		if condition() {
			return;
		}
		tokio::time::sleep(Duration::from_millis(10)).await;
	}
	panic!("the condition never held");
}

fn digest(tool: &Value) -> String {
	schema_digest(&tool["inputSchema"])
}

fn failure(error: Error) -> Option<ListFailure> {
	match error {
		Error::Port(source) => source.downcast_ref::<ListFailure>().copied(),
		_ => None,
	}
}

#[tokio::test]
async fn list_returns_the_tools_of_every_page() {
	let fixture = Fixture::start().await;

	let tools = fixture.list().await.unwrap();

	assert_eq!(
		tools,
		vec![
			ListedTool {
				name: "alpha".into(),
				description: Some("Alpha tool".into()),
				input_schema: alpha()["inputSchema"].clone(),
			},
			ListedTool {
				name: "beta".into(),
				description: None,
				input_schema: beta()["inputSchema"].clone(),
			},
		]
	);
}

#[tokio::test]
async fn one_session_serves_listing_and_calling() {
	let fixture = Fixture::start().await;
	let arguments = json!({"q": "rust"});

	fixture.list().await.unwrap();
	fixture.list().await.unwrap();
	let result = fixture
		.call("alpha", &digest(&alpha()), arguments.clone())
		.await
		.unwrap();

	assert_eq!(result["content"][0]["text"], arguments.to_string());
	assert_eq!(fixture.fake.count("initialize"), 1);
	assert_eq!(
		fixture.fake.sessions_used(),
		HashSet::from([Some("s1".to_owned())])
	);
	let sent = fixture.fake.call_params();
	assert_eq!(sent.len(), 1);
	assert_eq!(sent[0]["name"], "alpha");
	assert_eq!(sent[0]["arguments"], arguments);
}

#[tokio::test]
async fn closing_the_run_ends_the_session_and_the_next_call_starts_another() {
	let fixture = Fixture::start().await;
	fixture.list().await.unwrap();

	fixture.sessions.close(fixture.run).await;

	assert_eq!(fixture.fake.deleted(), ["s1"]);
	fixture
		.call("alpha", &digest(&alpha()), json!({}))
		.await
		.unwrap();
	assert_eq!(fixture.fake.count("initialize"), 2);
}

#[tokio::test]
async fn closing_a_run_that_never_opened_a_session_changes_nothing() {
	let fixture = Fixture::start().await;
	fixture.list().await.unwrap();

	fixture.sessions.close(Uuid::new_v4()).await;

	fixture.list().await.unwrap();
	assert_eq!(fixture.fake.count("initialize"), 1);
	assert!(fixture.fake.deleted().is_empty());
}

#[tokio::test]
async fn a_session_the_server_forgot_is_initialized_again_for_the_call() {
	let fixture = Fixture::start().await;
	fixture.list().await.unwrap();
	fixture.fake.forget_sessions();

	let result = fixture
		.call("alpha", &digest(&alpha()), json!({"q": "x"}))
		.await
		.unwrap();

	assert_eq!(result["isError"], false);
	assert_eq!(fixture.fake.count("initialize"), 2);
}

#[tokio::test]
async fn a_session_the_server_keeps_losing_is_opened_again_and_the_call_sent_once_more() {
	let fixture = Fixture::start().await;
	fixture.list().await.unwrap();
	// The server drops the session for the call, then again for the call that follows the
	// transparent re-initialization.
	fixture.fake.lost_calls.store(2, SeqCst);

	let result = fixture
		.call("alpha", &digest(&alpha()), json!({"q": "x"}))
		.await
		.unwrap();

	assert_eq!(result["isError"], false);
	assert_eq!(fixture.fake.count("initialize"), 3);
}

#[tokio::test]
async fn a_call_the_server_keeps_rejecting_is_not_sent_again_forever() {
	let fixture = Fixture::start().await;
	fixture.list().await.unwrap();
	fixture.fake.lost_calls.store(usize::MAX, SeqCst);

	let result = fixture
		.call("alpha", &digest(&alpha()), json!({"q": "x"}))
		.await;

	assert!(matches!(result, Err(Error::External(_))));
	assert!(fixture.fake.count("tools/call") <= 4);
}

#[tokio::test]
async fn a_call_that_failed_after_the_server_got_it_is_not_sent_again() {
	let fixture = Fixture::start().await;
	fixture.list().await.unwrap();
	fixture.fake.set_mode(Mode::BreakCall);

	let result = fixture
		.call("alpha", &digest(&alpha()), json!({"q": "x"}))
		.await;

	assert!(matches!(result, Err(Error::External(_))));
	assert_eq!(fixture.fake.count("tools/call"), 1);
}

#[tokio::test]
async fn a_reopened_session_refuses_a_tool_whose_schema_changed() {
	let fixture = Fixture::start().await;
	let pinned = digest(&alpha());
	fixture.list().await.unwrap();
	fixture.sessions.close(fixture.run).await;
	let changed = json!({
		"name": "alpha",
		"description": "Alpha tool",
		"inputSchema": {"type": "object", "properties": {"q": {"type": "integer"}}}
	});
	fixture.fake.set_tools(vec![changed.clone(), beta()]);

	for _ in 0..2 {
		let refused = fixture.call("alpha", &pinned, json!({"q": "x"})).await;
		assert!(
			matches!(&refused, Err(Error::Invalid(message)) if message == "CAPABILITY_CHANGED: alpha")
		);
	}
	let gone = fixture.call("gamma", &pinned, json!({})).await;
	assert!(
		matches!(&gone, Err(Error::Invalid(message)) if message == "CAPABILITY_CHANGED: gamma")
	);
	assert_eq!(fixture.fake.count("tools/call"), 0);

	fixture
		.call("alpha", &digest(&changed), json!({"q": 1}))
		.await
		.unwrap();
	assert_eq!(fixture.fake.count("tools/call"), 1);
}

#[tokio::test]
async fn an_mcp_error_result_is_returned_as_data() {
	let fixture = Fixture::start().await;
	fixture.list().await.unwrap();

	let result = fixture
		.call("alpha", &digest(&alpha()), json!({"fail": true}))
		.await
		.unwrap();

	assert_eq!(
		result,
		json!({"is_error": true, "content": [{"type": "text", "text": "boom"}]})
	);
}

#[tokio::test]
async fn arguments_must_be_an_object() {
	let fixture = Fixture::start().await;

	let result = fixture
		.call("alpha", &digest(&alpha()), json!("not an object"))
		.await;

	assert!(matches!(result, Err(Error::Invalid(_))));
	assert_eq!(fixture.fake.requests(), 0);
}

#[tokio::test]
async fn the_servers_headers_accompany_every_request() {
	let fixture = Fixture::start_with(
		&[("x-plugin", "demo"), ("X-Region", "eu")],
		IDLE_LIMIT,
		MAX_SESSIONS,
	)
	.await;

	fixture.list().await.unwrap();
	fixture
		.call("alpha", &digest(&alpha()), json!({}))
		.await
		.unwrap();

	let seen = fixture.fake.seen.lock().unwrap();
	let methods: Vec<_> = seen.iter().map(|seen| seen.method.as_str()).collect();
	assert_eq!(
		methods,
		[
			"initialize",
			"notifications/initialized",
			"tools/list",
			"tools/list",
			"tools/call"
		]
	);
	for seen in seen.iter() {
		assert_eq!(seen.headers["x-plugin"], "demo", "{}", seen.method);
		assert_eq!(seen.headers["x-region"], "eu", "{}", seen.method);
	}
}

#[tokio::test]
async fn an_origin_the_deployment_did_not_permit_is_refused_before_any_connection() {
	let (listener, origin) = bind().await;
	let sessions = sessions(&[], IDLE_LIMIT, MAX_SESSIONS);
	let config = remote(&origin, &[]);

	let listed = sessions
		.list(Uuid::new_v4(), &server("demo"), &config)
		.await;
	let called = sessions
		.call(
			Uuid::new_v4(),
			&server("demo"),
			&config,
			"alpha",
			"digest",
			json!({}),
			"key-1",
		)
		.await;

	assert!(matches!(listed, Err(Error::Forbidden)));
	assert!(matches!(called, Err(Error::Forbidden)));
	assert!(
		tokio::time::timeout(Duration::from_millis(200), listener.accept())
			.await
			.is_err(),
		"no connection may reach the listener"
	);
}

#[tokio::test]
async fn a_server_that_rejects_initialization_is_reported_as_initialize_failed() {
	for mode in [Mode::RejectInitialize, Mode::BreakInitialize] {
		let fixture = Fixture::start().await;
		fixture.fake.set_mode(mode);

		let listed = fixture.list().await;
		let called = fixture.call("alpha", &digest(&alpha()), json!({})).await;

		assert_eq!(
			failure(listed.unwrap_err()),
			Some(ListFailure::InitializeFailed)
		);
		assert!(matches!(called, Err(Error::External(_))));
	}
}

#[tokio::test]
async fn a_server_nobody_answers_for_is_reported_as_unreachable() {
	let (listener, origin) = bind().await;
	drop(listener);
	let sessions = sessions(&[&origin], IDLE_LIMIT, MAX_SESSIONS);

	let listed = sessions
		.list(Uuid::new_v4(), &server("demo"), &remote(&origin, &[]))
		.await;

	assert_eq!(failure(listed.unwrap_err()), Some(ListFailure::Unreachable));
}

#[tokio::test]
async fn a_listing_that_is_not_mcp_is_an_invalid_response() {
	let fixture = Fixture::start().await;
	fixture.fake.set_mode(Mode::GarbageList);

	let listed = fixture.list().await;

	assert_eq!(
		failure(listed.unwrap_err()),
		Some(ListFailure::InvalidResponse)
	);
}

#[tokio::test]
async fn a_listing_beyond_ten_pages_or_one_mebibyte_of_schemas_is_an_invalid_response() {
	for (mode, pages) in [
		(Mode::Endless { schema_bytes: 0 }, 10),
		(
			Mode::Endless {
				schema_bytes: 200_000,
			},
			6,
		),
	] {
		let fixture = Fixture::start().await;
		fixture.fake.set_mode(mode);

		let listed = fixture.list().await;

		assert_eq!(
			failure(listed.unwrap_err()),
			Some(ListFailure::InvalidResponse)
		);
		assert_eq!(fixture.fake.count("tools/list"), pages);
	}
}

#[tokio::test]
async fn a_session_idle_for_too_long_is_closed_and_opened_again() {
	let fixture = Fixture::start_with(&[], Duration::from_millis(100), MAX_SESSIONS).await;
	fixture.list().await.unwrap();

	tokio::time::sleep(Duration::from_millis(300)).await;
	fixture.list().await.unwrap();

	assert_eq!(fixture.fake.count("initialize"), 2);
	// Dropping the session ends it in the background.
	until(|| fixture.fake.deleted() == ["s1"]).await;
}

#[tokio::test]
async fn a_full_map_drops_the_least_recently_used_session() {
	let fixture = Fixture::start_with(&[], IDLE_LIMIT, 2).await;
	let (first, second, third) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
	for run in [first, second, third] {
		fixture.list_for(run).await.unwrap();
	}
	assert_eq!(fixture.fake.count("initialize"), 3);

	fixture.list_for(second).await.unwrap();
	fixture.list_for(third).await.unwrap();
	assert_eq!(
		fixture.fake.count("initialize"),
		3,
		"the newest sessions are kept"
	);

	fixture.list_for(first).await.unwrap();
	assert_eq!(
		fixture.fake.count("initialize"),
		4,
		"the oldest session was dropped"
	);
}

#[test]
fn sessions_idle_for_ten_minutes_are_dropped_and_at_most_512_are_kept() {
	let sessions = McpPluginSessions::new(KeylessAccess::default(), ToolClients::new().unwrap());

	let state = sessions.state();
	assert_eq!(state.idle, Duration::from_secs(600));
	assert_eq!(state.capacity, 512);
}

#[tokio::test]
async fn two_runs_never_share_a_session() {
	let fixture = Fixture::start().await;
	let (first, second) = (Uuid::new_v4(), Uuid::new_v4());

	fixture.list_for(first).await.unwrap();
	fixture.list_for(second).await.unwrap();

	assert_eq!(fixture.fake.count("initialize"), 2);
	assert_eq!(
		fixture.fake.sessions_used(),
		HashSet::from([Some("s1".to_owned()), Some("s2".to_owned())])
	);

	fixture.sessions.close(first).await;
	assert_eq!(fixture.fake.deleted(), ["s1"]);
	fixture
		.call_for(second, "alpha", &digest(&alpha()), json!({}))
		.await
		.unwrap();
	assert_eq!(
		fixture.fake.count("initialize"),
		2,
		"the other Run keeps its session"
	);
}
