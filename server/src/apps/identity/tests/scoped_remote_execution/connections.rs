//! Two live Nodes, a real encrypted Secret Store and a wiremock MCP endpoint.
use super::*;
use aidash_application::oauth_connections::Owner;
use aidash_domain::{
	agent_plugin::ConnectionCause,
	oauth_connections::{ClientRef, ConnectionState, OAuthConnection},
};
use aidash_server::tool::{PluginTool, Tool, ToolContext};
use reinhardt::query::{ExprTrait, QueryStatementBuilder};
use wiremock::{
	Mock, MockServer, Request, ResponseTemplate,
	matchers::{method, path},
};
#[allow(dead_code)] // Reuse the real Store setup; lifecycle helpers belong to other suites.
#[path = "../support/personal_credentials.rs"]
mod personal_credentials;

const CANARY: &str = "owner-only-oauth-canary";
async fn mcp() -> MockServer {
	let server = MockServer::start().await;
	Mock::given(method("POST")).and(path("/mcp")).respond_with(|request: &Request| {
        if request.headers.get("authorization").and_then(|h| h.to_str().ok()) != Some("Bearer owner-only-oauth-canary") {
            return ResponseTemplate::new(401).insert_header("www-authenticate", "Bearer");
        }
        let body: Value = request.body_json().unwrap();
        let result = match body["method"].as_str().unwrap_or_default() {
            "server/discover" => json!({"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":0,"cacheScope":"private"}),
            "initialize" => json!({"protocolVersion":body["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"home-only","version":"1"}}),
            "tools/list" => json!({"tools":[{"name":"lookup","inputSchema":{"type":"object"}},{"name":"interactive","inputSchema":{"type":"object"}}]}),
            "tools/call" => json!({"resultType":"complete","content":[{"type":"text","text":"home called"}]}),
            _ => return ResponseTemplate::new(202),
        };
        ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
    }).mount(&server).await;
	Mock::given(method("POST"))
		.and(path("/http"))
		.respond_with(|request: &Request| {
			assert_eq!(request.headers["authorization"], format!("Bearer {CANARY}"));
			let input: Value = request.body_json().unwrap();
			if input["shape"] == "failure" {
				return ResponseTemplate::new(500);
			}
			ResponseTemplate::new(200).set_body_json(if input["shape"] == "fallback" {
				json!({"outcome":"not_personal"})
			} else if input["shape"] == "null" {
				Value::Null
			} else {
				json!({"error":"connection_required","resource":"tool-owned-json"})
			})
		})
		.mount(&server)
		.await;
	server
}
async fn install(p: &mut Pair, resource: &str) -> (Value, Value) {
	p.a.store.keyless.permit_origin_of(resource).unwrap();
	p.b.store.keyless.permit_origin_of(resource).unwrap();
	let token = common::tenant_token(&p.b.store, "acme").await;
	let (_, mut agent) = request(
		&p.ba,
		&token,
		"GET",
		"/api/registry/research/1.0.0",
		Value::Null,
	)
	.await;
	agent
		.as_object_mut()
		.unwrap()
		.remove("binding_normalization");
	agent["id"] = json!("research-mcp");
	agent["config"]["bindings"].as_array_mut().unwrap().push(json!({"kind":"tool","target":{"registry_deployment":p.b.config.deployment,"id":"personal-mcp","version":"1.0.0"},"alias":"personal_mcp"}));
	let tool = json!({"id":"personal-mcp","version":"1.0.0","kind":"tool","name":{"en":"personal MCP"},"description":{"en":"owner connection"},"schema":{"type":"object"},"config":{"registry_deployment":p.b.config.deployment,"provider":"integration.mcp@1","operation":"invoke","default_alias":"personal_mcp","tier":"integration","transport":{"transport":"mcp","endpoint":resource,"tool_name":"lookup","replay":"idempotent","idempotency_argument":"request_id"}}});
	let mut http = tool.clone();
	http["id"] = json!("personal-http");
	http["config"]["operation"] = json!("invoke");
	http["config"]["provider"] = json!("integration.http@1");
	http["config"]["default_alias"] = json!("personal_http");
	http["config"]["transport"] = json!({"transport":"http","endpoint":resource.replace("/mcp", "/http"),"replay":"idempotent","secret":"personal-api","secret_owner":"personal"});
	agent["config"]["bindings"].as_array_mut().unwrap().push(json!({"kind":"tool","target":{"registry_deployment":p.b.config.deployment,"id":"personal-http","version":"1.0.0"},"alias":"personal_http"}));
	for entry in [tool.clone(), http.clone(), agent] {
		let id = entry["id"].clone();
		let (status, result) = request(&p.ba, &token, "POST", "/api/registry", entry).await;
		assert_eq!(status, 200, "{result}");
		let (status, result) = request(&p.ba, &token, "POST", "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true})).await;
		assert_eq!(status, 200, "{result}");
	}
	let subject = aidash_domain::agent_subject(&node_of(&p.b), "research-mcp", "1.0.0");
	for (f, app) in [(&p.a, &p.aa), (&p.b, &p.ba)] {
		let (_, mut policy) = request(
			app,
			&f.config.api_token,
			"GET",
			"/api/authorization/acme",
			Value::Null,
		)
		.await;
		policy["bundle"]["subjects"][&subject] = json!({"kind":"agent"});
		let (status, result) = request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":policy["revision"],"bundle":policy["bundle"]}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
	}
	let (status,result)=request(&p.aa,&p.token,"POST",&format!("/api/tasks/{}/remote-grants",p.task),json!({"id":p.grant,"node_id":node_of(&p.b),"agent":{"id":"research-mcp","version":"1.0.0"},"ttl_seconds":3600,"semantic":{"mode":"disabled"}})).await;
	assert_eq!(status, 200, "{result}");
	let (status, result) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{result}");
	p.admission = serde_json::from_value(result["admission_id"].clone()).unwrap();
	(tool, http)
}
async fn seed_connection(p: &Pair, resource: &str, subject: &str, expired: bool) -> Uuid {
	let service = p.a.store.oauth_connections.as_ref().unwrap();
	let owner = Owner {
		tenant: "acme".into(),
		subject: subject.into(),
	};
	let id = Uuid::now_v7();
	let secret = service.store.connection_resource(id);
	service.store.create_connection("acme", id).await.unwrap();
	let payload = json!({"access_token":CANARY,"refresh_token":null,"expires_at":null,"scope":["read"],"redirect_uri":"https://aidash.example/callback"});
	let version = service
		.store
		.add_version("acme", &secret, &payload.to_string().into())
		.await
		.unwrap();
	let now = chrono::Utc::now();
	let row = OAuthConnection {
		id,
		tenant: "acme".into(),
		subject: subject.into(),
		resource: resource.into(),
		issuer: "https://auth.example".into(),
		client: ClientRef::Dynamic {
			client_id: "client".into(),
		},
		scopes: vec!["read".into()],
		secret_resource: secret,
		pinned_version: version,
		state: ConnectionState::Active,
		access_expires_at: expired.then(|| now - chrono::Duration::seconds(120)),
		refresh_expires_at: None,
		created_at: now,
		updated_at: now,
		revision: 1,
	};
	let mut tx = service.repository.begin(&owner).await.unwrap();
	tx.save(&row).await.unwrap();
	tx.commit().await.unwrap();
	id
}
async fn peer(p: &Pair, operation: &str, data: Value) -> (u16, Value) {
	let payload =
		json!({"grant_id":p.grant,"admission_id":p.admission,"operation":operation,"data":data});
	let text = payload.to_string().to_lowercase();
	assert!(!text.contains(CANARY));
	assert!(!text.contains("authorization"));
	assert!(!text.contains("access_token"));
	let result = common::peer_request(
		&p.aa,
		(&p.b, "acme"),
		&node_of(&p.a).parse().unwrap(),
		"",
		"/federation/v0.1/scoped/execution/commands",
		payload,
	)
	.await;
	let text = result.1.to_string().to_lowercase();
	assert!(!text.contains(CANARY));
	assert!(!text.contains("authorization"));
	assert!(!text.contains("access_token"));
	result
}
async fn journal(p: &Pair) -> Vec<Value> {
	aidash_server::database::native::query_scalar(
		&Query::select()
			.expr(Expr::cust("to_jsonb(remote_connection_calls)"))
			.from(Alias::new("remote_connection_calls"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(p.a.store.control_pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap()
}
#[rstest::rstest]
#[tokio::test]
async fn remote_personal_mcp_calls_stay_at_home_and_replay_without_credentials_crossing(
	#[with(false, false, true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let mut p = scoped_pair;
	let mcp = mcp().await;
	let resource = format!("{}/mcp", mcp.uri());
	let store = personal_credentials::configure(&mut p.a).await;
	// The executor registers the personal-only HTTP tool with no shared row at all and holds
	// none of the person's values.
	let credentials = p.b.store.provider_credentials.clone();
	let reader = p.b.store.provider_key_material_reader.clone();
	personal_credentials::configure(&mut p.b).await;
	p.b.store.provider_credentials = credentials;
	p.b.store.provider_key_material_reader = reader;
	p.b.registry = p.b.registry.clone().with_tenant_secrets(true);
	p.ba.context.set_singleton(p.b.clone());
	let adapter = Arc::new(aidash_integrations::oauth::McpOAuth::new().unwrap());
	// Self-hosted Home: no broker settings, so Connections use the local reader.
	let settings: aidash_server::apps::identity::serializers::provider_credentials::Settings =
		serde_json::from_value(serde_json::json!({})).unwrap();
	aidash_server::bootstrap::configure_oauth_connections(
		&mut p.a.store,
		Some((adapter.clone(), adapter)),
		&settings,
		aidash_server::bootstrap::ProcessRole::Server,
	)
	.unwrap();
	p.aa.context.set_singleton(p.a.clone());
	let (entry, http) = install(&mut p, &resource).await;
	let run = p.run().await;
	let context = ToolContext {
		home: Home::new(p.b.clone(), run.clone()),
		store: p.b.store.clone(),
		run,
		scope: aidash_application::tenancy::TenancyScope::tenant("acme").unwrap(),
	};
	let tool = PluginTool {
		entry: serde_json::from_value(entry.clone()).unwrap(),
		alias: "personal_mcp".into(),
		config: serde_json::from_value(entry["config"]["transport"].clone()).unwrap(),
		transport: aidash_integrations::tools::HttpTools::new(
			p.b.store.keyless.clone(),
			p.b.store.tool_clients.clone(),
		),
	};
	// Bob's active Connection must not be used by Alice's remote Run.
	seed_connection(&p, &resource, "bob", false).await;
	let registry_data = json!({"tool_key":{"source":"registry","registry_deployment":p.b.config.deployment,"id":"personal-mcp","version":"1.0.0"},"resource":resource,"tool_name":"lookup","arguments":{"query":"private-argument-canary"},"idempotency_key":"retry-after-consent","kind":"connection"});
	let (status, missing) = peer(&p, "mcp.connection_call", registry_data.clone()).await;
	assert_eq!(status, 200, "{missing}");
	assert_eq!(missing, json!({"outcome":"not_personal"}));
	assert!(journal(&p).await.is_empty());
	// Expired-token resolution proves non-execution independently of transport challenges.
	let connection = seed_connection(&p, &resource, "alice", true).await;
	let (status, required) = peer(&p, "mcp.connection_call", registry_data.clone()).await;
	assert_eq!(status, 200, "{required}");
	assert_eq!(required["outcome"], "connection");
	assert_eq!(required["cause"]["error"], "connection_required");
	assert!(journal(&p).await.is_empty());
	// Recreate the Active-but-expired condition to exercise Node error restoration.
	let service = p.a.store.oauth_connections.as_ref().unwrap();
	let owner = Owner {
		tenant: "acme".into(),
		subject: "alice".into(),
	};
	let mut tx = service.repository.begin(&owner).await.unwrap();
	let mut row = tx
		.list()
		.await
		.unwrap()
		.into_iter()
		.find(|row| row.id == connection)
		.unwrap();
	row.state = ConnectionState::Active;
	tx.save(&row).await.unwrap();
	tx.commit().await.unwrap();
	assert!(matches!(
		tool.invoke(
			&context,
			json!({"query":"private-argument-canary"}),
			"retry-after-consent"
		)
		.await,
		Err(aidash_server::Error::Connection(
			ConnectionCause::ConnectionRequired { .. }
		))
	));
	assert!(journal(&p).await.is_empty());
	// A new consent can now retry the same logical key.
	let mut tx = service.repository.begin(&owner).await.unwrap();
	let mut row = tx
		.list()
		.await
		.unwrap()
		.into_iter()
		.find(|row| row.id == connection)
		.unwrap();
	row.state = ConnectionState::Active;
	row.access_expires_at = None;
	tx.save(&row).await.unwrap();
	tx.commit().await.unwrap();
	let called = tool
		.invoke(
			&context,
			json!({"query":"private-argument-canary"}),
			"retry-after-consent",
		)
		.await
		.unwrap();
	assert_eq!(called["content"][0]["text"], "home called");
	assert_eq!(
		tool.invoke(
			&context,
			json!({"query":"private-argument-canary"}),
			"retry-after-consent"
		)
		.await
		.unwrap(),
		called
	);
	let calls = mcp
		.received_requests()
		.await
		.unwrap()
		.into_iter()
		.filter(|r| {
			r.body_json::<Value>()
				.is_ok_and(|v| v["method"] == "tools/call")
		})
		.collect::<Vec<_>>();
	assert_eq!(calls.len(), 1);
	assert_eq!(
		calls[0].headers["authorization"],
		format!("Bearer {CANARY}")
	);
	let rows = journal(&p).await;
	assert_eq!(rows.len(), 1);
	assert_eq!(rows[0]["connection_id"], connection.to_string());
	assert_eq!(rows[0]["status"], "completed");
	let journal_text = serde_json::to_string(&rows).unwrap();
	assert!(!journal_text.contains(CANARY));
	assert!(!journal_text.contains("private-argument-canary"));
	assert!(!journal_text.contains("authorization"));
	let mut wrong = registry_data.clone();
	wrong["resource"] = json!("https://other.example/mcp");
	wrong["idempotency_key"] = json!("wrong-resource");
	assert_eq!(peer(&p, "mcp.connection_call", wrong).await.0, 403);
	// HTTP personal secrets also remain at Home; even control-shaped tool JSON is opaque.
	let secret =
		p.a.store
			.tenant_secrets
			.as_ref()
			.unwrap()
			.put_owned(
				"acme",
				Some("alice"),
				"personal-api",
				aidash_application::tenant_secrets::Put {
					expected_revision: 0,
					value: Some(CANARY.into()),
					hosts: Some(vec!["127.0.0.1".into()]),
				},
				"alice",
			)
			.await
			.unwrap();
	let http_tool = PluginTool {
		entry: serde_json::from_value(http.clone()).unwrap(),
		alias: "personal_http".into(),
		config: serde_json::from_value(http["config"]["transport"].clone()).unwrap(),
		transport: aidash_integrations::tools::HttpTools::new(
			p.b.store.keyless.clone(),
			p.b.store.tool_clients.clone(),
		),
	};
	for (shape, expected) in [
		("fallback", json!({"outcome":"not_personal"})),
		("null", Value::Null),
		("failure", json!({"error":"mcp_connection_call_failed"})),
		(
			"connection",
			json!({"error":"connection_required","resource":"tool-owned-json"}),
		),
	] {
		let key = format!("http-{shape}");
		assert_eq!(
			http_tool
				.invoke(&context, json!({"shape":shape}), &key)
				.await
				.unwrap(),
			expected
		);
		assert_eq!(
			http_tool
				.invoke(&context, json!({"shape":shape}), &key)
				.await
				.unwrap(),
			expected
		);
		let data = json!({"tool_key":{"source":"registry","registry_deployment":p.b.config.deployment,"id":"personal-http","version":"1.0.0"},"resource":http["config"]["transport"]["endpoint"],"tool_name":"","arguments":{"shape":shape},"idempotency_key":key,"kind":"personal_secret","secret_name":"personal-api"});
		let (status, reply) = peer(&p, "mcp.connection_call", data).await;
		assert_eq!(status, 200, "{reply}");
		assert_eq!(reply, json!({"outcome":"result","result":expected}));
	}
	let rows = journal(&p).await;
	let personal = rows
		.iter()
		.filter(|row| row["secret_id"] == secret.id.to_string())
		.collect::<Vec<_>>();
	assert_eq!(personal.len(), 4);
	for row in personal {
		let status = if row["idempotency_key"] == "http-failure" {
			"failed"
		} else {
			"completed"
		};
		assert_eq!(row["status"], status, "{row}");
		assert_eq!(row["secret_host"], "127.0.0.1");
	}

	assert_eq!(
		mcp.received_requests()
			.await
			.unwrap()
			.iter()
			.filter(|request| request.url.path() == "/http")
			.count(),
		4
	);
	// A committed send without a durable outcome must never execute again, even
	// if the outer command transaction was lost with its replay row.
	let mut tx =
		p.a.store
			.control_pool
			.begin(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_executor()
			.await
			.unwrap();
	let (sql, values) = Query::delete()
		.from_table(Alias::new("authorization_remote_commands"))
		.and_where(Expr::col("grant_id").eq(Expr::value(p.grant)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await
		.unwrap();
	let (sql, values) = Query::update()
		.table(Alias::new("remote_connection_calls"))
		.value(Alias::new("status"), "started")
		.value(Alias::new("result"), Option::<Value>::None)
		.and_where(Expr::col("idempotency_key").eq(Expr::value("retry-after-consent")))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, reinhardt::db::orm::execution::convert_values(values))
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let before = mcp.received_requests().await.unwrap().len();
	// SQL NULL and a completed JSON null result remain distinct on Home replay.
	assert_eq!(
		http_tool
			.invoke(&context, json!({"shape":"null"}), "http-null")
			.await
			.unwrap(),
		Value::Null
	);
	assert_eq!(
		http_tool
			.invoke(&context, json!({"shape":"failure"}), "http-failure")
			.await
			.unwrap(),
		json!({"error":"mcp_connection_call_failed"})
	);
	assert_eq!(mcp.received_requests().await.unwrap().len(), before);
	assert_eq!(
		tool.invoke(
			&context,
			json!({"query":"private-argument-canary"}),
			"retry-after-consent"
		)
		.await
		.unwrap()["error"],
		"mcp_call_outcome_unknown"
	);
	assert_eq!(mcp.received_requests().await.unwrap().len(), before);
	assert!(
		!store
			.versions("acme", &store.connection_resource(connection))
			.await
			.unwrap()
			.is_empty()
	);
	// Changing the source root to a service must bypass both personal lookup
	// and existing replay, while preserving the executor's shared/anonymous option.
	let (_, mut policy) = request(
		&p.aa,
		&p.a.config.api_token,
		"GET",
		"/api/authorization/acme",
		Value::Null,
	)
	.await;
	policy["bundle"]["subjects"]["alice"]["kind"] = json!("service");
	let (status, result) = request(
		&p.aa,
		&p.a.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":policy["revision"],"bundle":policy["bundle"]}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let rows = journal(&p).await;
	let requests = mcp.received_requests().await.unwrap().len();
	let (status, result) = peer(&p, "mcp.connection_call", registry_data).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result, json!({"outcome":"not_personal"}));
	let (status, unavailable) = peer(&p, "mcp.connection_call", json!({"tool_key":{"source":"registry","registry_deployment":p.b.config.deployment,"id":"personal-http","version":"1.0.0"},"resource":http["config"]["transport"]["endpoint"],"tool_name":"","arguments":{"shape":"fallback"},"idempotency_key":"http-fallback","kind":"personal_secret","secret_name":"personal-api"})).await;
	assert_eq!(status, 200, "{unavailable}");
	// A personal-only tool has no shared or anonymous option to fall back to: a Run that
	// no person started is refused by name, and nothing is sent or journaled.
	assert_eq!(
		unavailable,
		json!({"outcome":"result","result":{"error":"secret_unavailable","resource":http["config"]["transport"]["endpoint"]}})
	);
	assert_eq!(journal(&p).await, rows);
	assert_eq!(mcp.received_requests().await.unwrap().len(), requests);
	p.close().await;
}

/// Home's MCP server refuses every request with a real Bearer challenge.
async fn mcp_refusing() -> MockServer {
	let server = MockServer::start().await;
	Mock::given(method("POST"))
		.and(path("/mcp"))
		.respond_with(ResponseTemplate::new(401).insert_header(
			"www-authenticate",
			"Bearer resource_metadata=\"https://mcp.example/.well-known/oauth-protected-resource\"",
		))
		.mount(&server)
		.await;
	server
}

#[rstest::rstest]
#[tokio::test]
async fn a_real_401_at_home_crosses_the_envelope_as_a_typed_connection_cause(
	#[with(false, false, true)]
	#[future(awt)]
	scoped_pair: Pair,
) {
	let mut p = scoped_pair;
	let mcp = mcp_refusing().await;
	let resource = format!("{}/mcp", mcp.uri());
	personal_credentials::configure(&mut p.a).await;
	let credentials = p.b.store.provider_credentials.clone();
	let reader = p.b.store.provider_key_material_reader.clone();
	personal_credentials::configure(&mut p.b).await;
	p.b.store.provider_credentials = credentials;
	p.b.store.provider_key_material_reader = reader;
	p.b.registry = p.b.registry.clone().with_tenant_secrets(true);
	p.ba.context.set_singleton(p.b.clone());
	let adapter = Arc::new(aidash_integrations::oauth::McpOAuth::new().unwrap());
	let settings: aidash_server::apps::identity::serializers::provider_credentials::Settings =
		serde_json::from_value(serde_json::json!({})).unwrap();
	aidash_server::bootstrap::configure_oauth_connections(
		&mut p.a.store,
		Some((adapter.clone(), adapter)),
		&settings,
		aidash_server::bootstrap::ProcessRole::Server,
	)
	.unwrap();
	p.aa.context.set_singleton(p.a.clone());
	let (entry, _) = install(&mut p, &resource).await;
	let run = p.run().await;
	let context = ToolContext {
		home: Home::new(p.b.clone(), run.clone()),
		store: p.b.store.clone(),
		run,
		scope: aidash_application::tenancy::TenancyScope::tenant("acme").unwrap(),
	};
	let tool = PluginTool {
		entry: serde_json::from_value(entry.clone()).unwrap(),
		alias: "personal_mcp".into(),
		config: serde_json::from_value(entry["config"]["transport"].clone()).unwrap(),
		transport: aidash_integrations::tools::HttpTools::new(
			p.b.store.keyless.clone(),
			p.b.store.tool_clients.clone(),
		),
	};
	// Alice is connected and her token is current, so only the server can refuse it.
	seed_connection(&p, &resource, "alice", false).await;
	let data = json!({"tool_key":{"source":"registry","registry_deployment":p.b.config.deployment,"id":"personal-mcp","version":"1.0.0"},"resource":resource,"tool_name":"lookup","arguments":{"query":"private-argument-canary"},"idempotency_key":"real-401","kind":"connection"});
	let (status, reply) = peer(&p, "mcp.connection_call", data).await;
	assert_eq!(status, 200, "{reply}");
	assert_eq!(reply["outcome"], "connection", "{reply}");
	assert_eq!(reply["cause"]["error"], "connection_required");
	assert_eq!(
		reply["cause"]["resource"],
		aidash_domain::oauth_connections::canonical_resource(&resource).unwrap()
	);
	// The executing Node restores the same typed cause, not a tool result.
	let Err(aidash_server::Error::Connection(ConnectionCause::ConnectionRequired {
		resource: refused,
	})) = tool
		.invoke(
			&context,
			json!({"query":"private-argument-canary"}),
			"real-401-executor",
		)
		.await
	else {
		panic!("the executing Node must get a typed connection refusal");
	};
	assert_eq!(
		refused,
		aidash_domain::oauth_connections::canonical_resource(&resource).unwrap()
	);
	// The refusal happened at the auth layer: no operation was sent.
	assert!(mcp.received_requests().await.unwrap().iter().all(|r| {
		r.body_json::<Value>()
			.is_ok_and(|v| v["method"] != "tools/call")
	}));
	p.close().await;
}
