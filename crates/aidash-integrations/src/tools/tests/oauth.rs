//! Protocol proof at the real HTTP boundary, shared by Registry and Agent Plugin MCP.
use super::*;
use aidash_application::ports::tools::{PluginServer, PluginSessions};
use aidash_application::tenant_secrets::{BearerSource, ConnectionRecord};
use aidash_domain::agent_plugin::{RemoteServer, schema_digest};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use wiremock::{
	Mock, MockServer, Request, ResponseTemplate,
	matchers::{method, path},
};

#[derive(Default)]
struct Tokens {
	generation: AtomicUsize,
	invalidations: AtomicUsize,
}
#[async_trait]
impl BearerSource for Tokens {
	async fn bearer(&self) -> Result<SecretString> {
		Ok(format!("token-{}", self.generation.load(SeqCst)).into())
	}
	async fn invalidate(&self) {
		self.invalidations.fetch_add(1, SeqCst);
		self.generation.fetch_add(1, SeqCst);
	}
}
fn connection(endpoint: &str, source: Arc<Tokens>) -> CallSecret {
	CallSecret::Connection {
		record: ConnectionRecord {
			id: uuid::Uuid::now_v7(),
			resource: endpoint.into(),
			host: "127.0.0.1".into(),
		},
		source,
	}
}
fn config(endpoint: String) -> ToolConfig {
	ToolConfig::Mcp {
		endpoint,
		tool_name: "lookup".into(),
		replay: "idempotent".into(),
		idempotency_argument: Some("request_id".into()),
		secret: None,
		scopes: None,
		secret_owner: Default::default(),
	}
}
async fn upstream(
	refuse_at: &'static str,
	status: u16,
	permanent: bool,
	modern: bool,
) -> MockServer {
	let server = MockServer::start().await;
	Mock::given(method("POST")).and(path("/mcp")).respond_with(move |request: &Request| {
		let message: Value = request.body_json().unwrap();
		let method = message["method"].as_str().unwrap_or_default();
		let bearer = request.headers.get("authorization").and_then(|h| h.to_str().ok());
		if refuse_at == "continuation" && method == "tools/call" {
			if message["params"].get("requestState").is_some() {
				return ResponseTemplate::new(401).insert_header("www-authenticate", "Bearer");
			}
			return ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":message["id"],"result":{
				"resultType":"input_required","requestState":"accepted-operation","inputRequests":{}}}));
		}

		if (refuse_at == "all" || method == refuse_at) && (permanent || bearer != Some("Bearer token-1")) {
			return ResponseTemplate::new(status).insert_header("www-authenticate",
				if status == 403 { "Bearer error=\"insufficient_scope\", scope=\"files.read files.write\"" } else { "Bearer resource_metadata=\"https://mcp.example/.well-known/oauth-protected-resource\"" });
		}
		let result = match method {
			"server/discover" if modern => json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"}),
			"server/discover" => return ResponseTemplate::new(400).set_body_json(json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"legacy server"}})),
			"initialize" => json!({"protocolVersion":message["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"wiremock","version":"1"}}),
			"tools/list" => json!({"tools":[{"name":"lookup","inputSchema":{"type":"object"}}]}),
			"tools/call" if modern => json!({"resultType":"complete","content":[{"type":"text","text":"authenticated"}]}),
			"tools/call" => json!({"content":[{"type":"text","text":"authenticated"}]}),
			_ => return ResponseTemplate::new(202),
		};
		ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":message["id"],"result":result}))
	}).mount(&server).await;
	server
}

#[rstest::rstest]
#[case::initialize("initialize", 401, "connection_required")]
#[case::call("tools/call", 401, "connection_required")]
#[case::scope("tools/call", 403, "connection_scope_insufficient")]
#[tokio::test]
async fn registry_mcp_challenges_are_typed_refusals(
	#[case] phase: &'static str,
	#[case] status: u16,
	#[case] code: &str,
) {
	let server = upstream(phase, status, true, false).await;
	let endpoint = format!("{}/mcp", server.uri());
	let tools = HttpTools::new(permitting([endpoint.as_str()]), ToolClients::new().unwrap());
	let error = tools
		.invoke(&config(endpoint.clone()), json!({}), "call-key", None)
		.await
		.unwrap_err();
	assert_connection(
		error,
		code,
		&endpoint,
		(status == 403).then_some("files.read files.write"),
	);

	assert!(
		server
			.received_requests()
			.await
			.unwrap()
			.iter()
			.all(|request| !request.headers.contains_key("authorization"))
	);
}

#[rstest::rstest]
#[case::initialize("initialize", false)]
#[case::call("tools/call", false)]
#[case::second_refusal("tools/call", true)]
#[tokio::test]
async fn registry_mcp_refreshes_once_and_restarts_the_call(
	#[case] phase: &'static str,
	#[case] permanent: bool,
) {
	let server = upstream(phase, 401, permanent, false).await;
	let endpoint = format!("{}/mcp", server.uri());
	let source = Arc::new(Tokens::default());
	let auth = connection(&endpoint, source.clone());
	let tools = HttpTools::new(permitting([endpoint.as_str()]), ToolClients::new().unwrap());
	let result = tools
		.invoke(
			&config(endpoint.clone()),
			json!({}),
			"call-key",
			Some(&auth),
		)
		.await;
	if permanent {
		assert_connection(result.unwrap_err(), "connection_required", &endpoint, None);
	} else {
		assert_eq!(result.unwrap()["content"][0]["text"], "authenticated");
	}
	assert_eq!(source.invalidations.load(SeqCst), 1);
	let requests = server.received_requests().await.unwrap();
	let calls: Vec<_> = requests
		.iter()
		.filter(|r| r.body_json::<Value>().unwrap()["method"] == phase)
		.collect();
	assert_eq!(calls.len(), 2);
	assert_eq!(calls[0].headers["authorization"], "Bearer token-0");
	assert_eq!(calls[1].headers["authorization"], "Bearer token-1");
	for request in requests
		.iter()
		.filter(|r| r.body_json::<Value>().unwrap()["method"] == "tools/call")
	{
		assert_eq!(
			request.body_json::<Value>().unwrap()["params"]["arguments"]["request_id"],
			"call-key"
		);
	}
}

#[rstest::rstest]
#[case::modern(true)]
#[case::legacy(false)]
#[tokio::test]
async fn plugin_cached_session_reads_the_current_bearer_for_every_request(#[case] modern: bool) {
	let server = upstream("never", 401, false, modern).await;
	let endpoint = format!("{}/mcp", server.uri());
	let source = Arc::new(Tokens::default());
	let auth = connection(&endpoint, source.clone());
	let sessions = crate::plugin_sessions::McpPluginSessions::new(
		permitting([endpoint.as_str()]),
		ToolClients::new().unwrap(),
	);
	let run = uuid::Uuid::new_v4();
	let identity = PluginServer {
		plugin: "demo".to_owned().try_into().unwrap(),
		digest: "copy".into(),
		server: "search".to_owned().try_into().unwrap(),
	};
	let remote = RemoteServer {
		url: endpoint.parse().unwrap(),
		headers: vec![],
	};
	let listed = sessions
		.list(run, &identity, &remote, Some(&auth))
		.await
		.unwrap();
	let digest = schema_digest(&listed[0].input_schema);
	for generation in [0, 1] {
		source.generation.store(generation, SeqCst);
		let result = sessions
			.call(
				run,
				&identity,
				&remote,
				"lookup",
				&digest,
				json!({}),
				"call",
				Some(&auth),
			)
			.await
			.unwrap();
		assert_eq!(result["content"][0]["text"], "authenticated");
	}
	let requests = server.received_requests().await.unwrap();
	let calls: Vec<_> = requests
		.iter()
		.filter(|r| r.body_json::<Value>().unwrap()["method"] == "tools/call")
		.collect();
	assert_eq!(calls.len(), 2);
	assert_eq!(calls[0].headers["authorization"], "Bearer token-0");
	assert_eq!(calls[1].headers["authorization"], "Bearer token-1");
	assert_eq!(
		requests
			.iter()
			.filter(|r| matches!(
				r.body_json::<Value>().unwrap()["method"].as_str(),
				Some("initialize" | "server/discover")
			))
			.count(),
		if modern { 1 } else { 2 }
	);
	sessions.close(run).await;
}

#[rstest::rstest]
#[case::missing_initialize("all", 401, false, true, "connection_required")]
#[case::missing_call("tools/call", 401, false, true, "connection_required")]
#[case::refresh_initialize("all", 401, true, false, "")]
#[case::refresh_call("tools/call", 401, true, false, "")]
#[case::second_refusal("tools/call", 401, true, true, "connection_required")]
#[case::scope("tools/call", 403, true, true, "connection_scope_insufficient")]
#[tokio::test]
async fn plugin_mcp_auth_challenges_preserve_context_and_retry_only_one_refused_bearer(
	#[case] phase: &'static str,
	#[case] status: u16,
	#[case] connected: bool,
	#[case] permanent: bool,
	#[case] code: &str,
) {
	let server = upstream(phase, status, permanent, true).await;
	let endpoint = format!("{}/mcp", server.uri());
	let source = Arc::new(Tokens::default());
	let auth = connected.then(|| connection(&endpoint, source.clone()));
	let sessions = crate::plugin_sessions::McpPluginSessions::new(
		permitting([endpoint.as_str()]),
		ToolClients::new().unwrap(),
	);
	let run = uuid::Uuid::new_v4();
	let identity = PluginServer {
		plugin: "demo".to_owned().try_into().unwrap(),
		digest: "copy".into(),
		server: "search".to_owned().try_into().unwrap(),
	};
	let remote = RemoteServer {
		url: endpoint.parse().unwrap(),
		headers: vec![],
	};
	let digest = schema_digest(&json!({"type":"object"}));
	let result = sessions
		.call(
			run,
			&identity,
			&remote,
			"lookup",
			&digest,
			json!({}),
			"call",
			auth.as_ref(),
		)
		.await;
	if code.is_empty() {
		assert_eq!(result.unwrap()["content"][0]["text"], "authenticated");
	} else {
		assert_connection(
			result.unwrap_err(),
			code,
			&endpoint,
			(status == 403).then_some("files.read files.write"),
		);
	}
	assert_eq!(
		source.invalidations.load(SeqCst),
		usize::from(connected && status == 401)
	);
	sessions.close(run).await;
}

#[tokio::test]
async fn revoked_consent_before_sending_is_a_structured_error_without_invalidating_or_sending() {
	struct Revoked;
	#[async_trait]
	impl BearerSource for Revoked {
		async fn bearer(&self) -> Result<SecretString> {
			Err(Error::Invalid("connection_required".into()))
		}
		async fn invalidate(&self) {
			panic!("no server refused a token");
		}
	}
	let server = upstream("never", 401, false, false).await;
	let endpoint = format!("{}/mcp", server.uri());
	let auth = CallSecret::Connection {
		record: ConnectionRecord {
			id: uuid::Uuid::now_v7(),
			resource: endpoint.clone(),
			host: "127.0.0.1".into(),
		},
		source: Arc::new(Revoked),
	};
	let tools = HttpTools::new(permitting([endpoint.as_str()]), ToolClients::new().unwrap());
	assert_connection(
		tools
			.invoke(&config(endpoint.clone()), json!({}), "key", Some(&auth))
			.await
			.unwrap_err(),
		"connection_required",
		&endpoint,
		None,
	);
	assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_connection_cannot_send_its_bearer_to_another_resource() {
	let server = upstream("never", 401, false, false).await;
	let endpoint = format!("{}/mcp", server.uri());
	let auth = connection("https://other.example/mcp", Arc::new(Tokens::default()));
	let tools = HttpTools::new(permitting([endpoint.as_str()]), ToolClients::new().unwrap());
	assert!(matches!(
		tools
			.invoke(&config(endpoint), json!({}), "key", Some(&auth))
			.await,
		Err(Error::Forbidden)
	));
	assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn plugin_auth_refusal_after_an_accepted_input_round_is_not_replayed() {
	use aidash_domain::agent_plugin::interaction::{CallCheckpoint, ToolRound};
	let server = upstream("continuation", 401, true, true).await;
	let endpoint = format!("{}/mcp", server.uri());
	let source = Arc::new(Tokens::default());
	let auth = connection(&endpoint, source.clone());
	let sessions = crate::plugin_sessions::McpPluginSessions::new(
		permitting([endpoint.as_str()]),
		ToolClients::new().unwrap(),
	);
	let run = uuid::Uuid::new_v4();
	let identity = PluginServer {
		plugin: "demo".to_owned().try_into().unwrap(),
		digest: "copy".into(),
		server: "search".to_owned().try_into().unwrap(),
	};
	let remote = RemoteServer {
		url: endpoint.parse().unwrap(),
		headers: vec![],
	};
	let digest = schema_digest(&json!({"type":"object"}));
	let first = sessions
		.round(
			run,
			&identity,
			&remote,
			"lookup",
			&digest,
			json!({}),
			"call",
			None,
			None,
			Some(&auth),
		)
		.await
		.unwrap();
	let ToolRound::InputRequired(input) = first else {
		panic!("accepted input round expected");
	};
	let mut checkpoint = CallCheckpoint::new("call".into());
	checkpoint.accept(input).unwrap();
	let result = sessions
		.round(
			run,
			&identity,
			&remote,
			"lookup",
			&digest,
			json!({}),
			"call",
			Some(&checkpoint),
			None,
			Some(&auth),
		)
		.await;
	assert!(
		matches!(result, Err(Error::External(_))),
		"an accepted operation must stay an uncertain MCP failure: {result:?}"
	);
	assert_eq!(source.invalidations.load(SeqCst), 0);
	let requests = server.received_requests().await.unwrap();
	assert_eq!(
		requests
			.iter()
			.filter(|r| r.body_json::<Value>().unwrap()["method"] == "tools/call")
			.count(),
		2
	);
	sessions.close(run).await;
}

fn assert_connection(error: Error, code: &str, resource: &str, scope: Option<&str>) {
	let Error::Connection(cause) = error else {
		panic!("expected typed connection refusal: {error:?}");
	};
	assert_eq!(cause.code(), code);
	assert_eq!(cause.resource(), resource);
	if let aidash_domain::agent_plugin::ConnectionCause::ConnectionScopeInsufficient {
		scope: actual,
		..
	} = cause
	{
		assert_eq!(actual.as_deref(), scope);
	}
}

#[rstest::rstest]
#[case::successful_body(200)]
#[case::real_unauthorized(401)]
#[case::real_scope_refusal(403)]
#[tokio::test]
async fn http_connection_refusals_come_from_status_never_from_a_success_body(#[case] status: u16) {
	let server = MockServer::start().await;
	let endpoint = format!("{}/tool", server.uri());
	let body = json!({"error":"connection_required","resource":endpoint});
	Mock::given(method("POST"))
		.and(path("/tool"))
		.respond_with(
			ResponseTemplate::new(status)
				.insert_header(
					"www-authenticate",
					if status == 403 {
						"Bearer error=\"insufficient_scope\", scope=\"files.read\""
					} else {
						"Bearer"
					},
				)
				.set_body_json(body.clone()),
		)
		.mount(&server)
		.await;
	let tools = HttpTools::new(permitting([endpoint.as_str()]), ToolClients::new().unwrap());
	let configuration = ToolConfig::Http {
		endpoint: endpoint.clone(),
		replay: "unsafe".into(),
		secret: None,
		secret_owner: Default::default(),
	};
	let result = tools.invoke(&configuration, json!({}), "key", None).await;
	if status == 200 {
		assert_eq!(result.unwrap(), body);
	} else {
		assert_connection(
			result.unwrap_err(),
			if status == 401 {
				"connection_required"
			} else {
				"connection_scope_insufficient"
			},
			&endpoint,
			(status == 403).then_some("files.read"),
		);
	}
	assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
