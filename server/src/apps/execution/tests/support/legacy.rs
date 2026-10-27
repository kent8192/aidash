use aidash_server::{
	authorization::Authorization,
	config::Config,
	domain::qualified_agent,
	federation::Federation,
	registry::{EntityRef, Registry},
	store::Store,
};
use reinhardt::db::backends::{DatabaseConnection, dialect::PostgresBackend};
use reinhardt::db::migrations::{FilesystemSource, MigrationSource};
use reinhardt::test::fixtures::http_client;
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{path::PathBuf, str::FromStr, sync::Arc};
use uuid::Uuid;

/// Wait until database work is blocked by this fixture's lock, including a
/// waiter queued behind another blocked transaction.
#[allow(dead_code)] // Only concurrency regressions need a database lock barrier.
pub async fn wait_for_blocked(pool: &aidash_server::database::native::Pool, pid: i32, count: i64) {
	use aidash_application::tenancy::TEST_FIXTURE;
	use reinhardt::query::{
		Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
	};
	let query = Query::select()
		.expr(Expr::cust("COUNT(*)"))
		.from(Alias::new("pg_stat_activity"))
		.and_where(SimpleExpr::CustomWithExpr(
			"? = ANY(pg_blocking_pids(pid)) OR EXISTS (SELECT 1 FROM unnest(pg_blocking_pids(pid)) AS blocker WHERE ? = ANY(pg_blocking_pids(blocker)))".into(),
			vec![Expr::value(pid).into(), Expr::value(pid).into()],
		))
		.to_string(PostgresQueryBuilder);
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			let blocked: i64 = pool
				.atomic(&TEST_FIXTURE, async |tx| {
					aidash_server::database::native::query_scalar(&query)
						.scalar_one(tx)
						.await
				})
				.await
				.unwrap();
			if blocked >= count {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
	})
	.await
	.expect("the HTTP operation reached the fixture's database lock");
}

#[allow(dead_code)] // Browser-authentication suites seed current-format sessions directly.
pub fn browser_session_digest(value: impl AsRef<[u8]>) -> Vec<u8> {
	use sha2::Digest as _;

	let mut digest = sha2::Sha256::new();
	digest.update(b"aidash-browser-session-v2\0");
	digest.update(value.as_ref());
	digest.finalize().to_vec()
}

#[path = "environment.rs"]
mod environment;
#[allow(unused_imports)] // Only suites whose tests have hung use the whole-test backstop.
pub use environment::TEST_BOUND;
#[allow(unused_imports)] // Sixteen-Node acceptance cases need a separate connection budget.
pub use environment::isolated_test_environment;
use environment::postgres;
#[allow(unused_imports)] // Only fault-injection suites change the schema.
pub use environment::postgres::{admin_pool, owner_pool};
pub use environment::{TestEnvironment, test_environment};
#[path = "application.rs"]
mod application;
#[allow(unused_imports)] // Each binary uses only its required fixture constructors.
pub use application::{
	ApplicationFixture, ApplicationFuture, RouterTransform, TestApplication, application,
	application_with, application_with_event_streams, application_with_settings,
	direct_application, native_application, streaming_http_client,
};
#[path = "peer.rs"]
mod peer;
#[path = "federation.rs"]
pub mod peering;
#[path = "upstream.rs"]
pub mod upstream_fixtures;
#[allow(unused_imports)] // Worker-process fixtures share this settings module.
pub(crate) use application::{process_settings, settings_for};
#[allow(unused_imports)] // Only federation suites consume fixed-origin application fixtures.
pub use peer::{FixedServerGuard as PeerServerGuard, PeerFixture, PeerFuture, native_peer};

#[path = "tenant_roots.rs"]
mod tenant_roots;
#[allow(unused_imports)] // Only suites that create Tenants under fixed names use these.
pub use tenant_roots::{
	admitted_run, approve_agent, approve_agent_on, default_node, local_run, remote_run,
	remote_run_in, replace_policy, second_node_in, subject_claim, tenant_root, tenant_root_in,
	tenant_scope, tenant_token,
};

#[allow(dead_code)] // Header-sensitive browser and peer tests share this request dispatcher.
pub async fn http_response(
	app: &TestApplication,
	method: &str,
	path: &str,
	headers: &[(&str, &str)],
	body: &[u8],
) -> reinhardt::test::TestResponse {
	let client = &(app.client());
	let mut request = client
		.request(method.parse().unwrap(), path)
		.body(bytes::Bytes::copy_from_slice(body));
	for (name, value) in headers {
		request = request.header(*name, *value);
	}
	request.send().await.unwrap()
}

#[allow(dead_code)] // Shared fixtures are used by different integration-test binaries.
pub async fn request(
	app: &TestApplication,
	token: &str,
	method: &str,
	path: &str,
	value: Value,
) -> (u16, Value) {
	let mut request = app
		.api_http
		.request(method.parse().unwrap(), path)
		.header("Authorization", format!("Bearer {token}"));
	if matches!(method, "POST" | "PUT" | "PATCH") || (method == "DELETE" && !value.is_null()) {
		request = request.json(&value);
	}
	let response = request.send().await.unwrap();
	json_response(method, path, response.status_code(), response.body())
}

/// Registers `other`'s default Node of `other_tenant` at `local`'s default Node
/// of `tenant`, through the registration route with the Deployment
/// Administrator's token. `other` must be serving, because the route fetches
/// its `/.well-known/aidash`.
#[allow(dead_code)]
pub async fn register_default_node(
	local: (&Federation, &TestApplication, &str),
	other: (&Federation, &str),
) {
	let (f, app, tenant) = local;
	let (other, other_tenant) = other;
	let node = default_node(&f.store, tenant).await;
	let other_node = default_node(&other.store, other_tenant).await;
	peering::permit(f, &other.config.endpoint);
	let (status, body) = request(
		app,
		&f.config.api_token,
		"POST",
		&peering::registration_path(tenant, node.node.as_str()),
		peering::registration(
			&other_node.to_string(),
			Some((&other.config.endpoint, &peering::key_id(other))),
		),
	)
	.await;
	assert_eq!(status, 200, "{body}");
}

/// Registers the default Node of `source_tenant`, another Tenant of this
/// Deployment, at `tenant`'s default Node, and returns the registered address.
#[allow(dead_code)]
pub async fn register_local_node(
	f: &Federation,
	app: &TestApplication,
	tenant: &str,
	source_tenant: &str,
) -> aidash_domain::nodes::NodeAddress {
	tenant_root(f, source_tenant).await;
	let node = default_node(&f.store, tenant).await;
	let source = default_node(&f.store, source_tenant).await;
	let (status, body) = request(
		app,
		&f.config.api_token,
		"POST",
		&peering::registration_path(tenant, node.node.as_str()),
		peering::registration(&source.to_string(), None),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	source
}

/// Registers `other`'s default Node at `local`'s although `other` serves
/// nothing, so the registration only lets `local` accept it: its endpoint is
/// a stand-in that answers the identity request and nothing else.
#[allow(dead_code)]
pub async fn register_sender_only(
	local: (&Federation, &TestApplication, &str),
	other: (&Federation, &str),
) {
	let (f, app, tenant) = local;
	let (other, other_tenant) = other;
	let stand_in =
		peering::Remote::standing_in(other.config.deployment.as_str(), peering::key_of(other))
			.await;
	let node = default_node(&f.store, tenant).await;
	let other_node = default_node(&other.store, other_tenant).await;
	peering::permit(f, &stand_in.endpoint);
	let (status, body) = request(
		app,
		&f.config.api_token,
		"POST",
		&peering::registration_path(tenant, node.node.as_str()),
		stand_in.registration(other_node.node.as_str()),
	)
	.await;
	assert_eq!(status, 200, "{body}");
}

/// Replaces the registration at `local`'s default Node of `tenant` for `other`'s
/// default Node, which currently holds `revision`, with one that is `enabled`.
/// A disabling request keeps what the registration pinned; an enabling one
/// pins what `other` publishes now.
#[allow(dead_code)]
pub async fn set_node_enabled(
	local: (&Federation, &TestApplication, &str),
	other: (&Federation, &str),
	enabled: bool,
	revision: i64,
) {
	let (f, app, tenant) = local;
	let (other, other_tenant) = other;
	let node = default_node(&f.store, tenant).await;
	let other_node = default_node(&other.store, other_tenant).await;
	let mut body = peering::registration(
		&other_node.to_string(),
		Some((&other.config.endpoint, &peering::key_id(other))),
	);
	body["enabled"] = json!(enabled);
	body["expected_revision"] = json!(revision);
	let (status, response) = request(
		app,
		&f.config.api_token,
		"POST",
		&peering::registration_path(tenant, node.node.as_str()),
		body,
	)
	.await;
	assert_eq!(status, 200, "{response}");
}

/// Replaces the registration at `local`'s default Node of `tenant` for `other`'s
/// default Node, which currently holds `revision`, so it addresses `endpoint`.
/// The Deployment at `endpoint` must publish `other`'s key.
#[allow(dead_code)]
pub async fn repoint_node(
	local: (&Federation, &TestApplication, &str),
	other: (&Federation, &str),
	endpoint: &str,
	revision: i64,
) {
	let (f, app, tenant) = local;
	let (other, other_tenant) = other;
	let node = default_node(&f.store, tenant).await;
	let other_node = default_node(&other.store, other_tenant).await;
	peering::permit(f, endpoint);
	let mut body = peering::registration(
		&other_node.to_string(),
		Some((endpoint, &peering::key_id(other))),
	);
	body["expected_revision"] = json!(revision);
	let (status, response) = request(
		app,
		&f.config.api_token,
		"POST",
		&peering::registration_path(tenant, node.node.as_str()),
		body,
	)
	.await;
	assert_eq!(status, 200, "{response}");
}

/// A stand-in Deployment that `tenant`'s default Node of the receiving
/// Deployment accepts, and the Node of it that sends.
#[allow(dead_code)]
pub struct Source {
	pub remote: peering::Remote,
	pub node: String,
	pub receiver: aidash_domain::nodes::NodeAddress,
}

#[allow(dead_code)]
impl Source {
	/// The address of the sending Node.
	pub fn address(&self) -> String {
		self.remote.node(&self.node)
	}
}

/// Registers a Node of the stand-in Deployment `deployment` at `tenant`'s
/// default Node, through the registration route.
#[allow(dead_code)]
pub async fn register_source(
	f: &Federation,
	app: &TestApplication,
	deployment: &str,
	tenant: &str,
) -> Source {
	let remote = peering::Remote::start(deployment).await;
	let node = aidash_domain::nodes::NodeName::derived(&format!("{deployment}-sender")).to_string();
	let receiver = default_node(&f.store, tenant).await;
	peering::permit(f, &remote.endpoint);
	let (status, body) = request(
		app,
		&f.config.api_token,
		"POST",
		&peering::registration_path(tenant, receiver.node.as_str()),
		remote.registration(&node),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	Source {
		remote,
		node,
		receiver,
	}
}

/// POSTs a Peer Graph View request. A subject viewer is a Node's request as
/// that subject; a Deployment Administrator viewer has no Node, names the
/// receiving Deployment and acts under a Peer Graph Grant.
#[allow(dead_code)]
pub async fn post_graph(
	app: &TestApplication,
	source: &Source,
	viewer: &Value,
	payload: &Value,
) -> (u16, Value) {
	use peering::Bearer;
	let (receiver, bearer, subject) = if viewer["kind"] == "deployment_administrator" {
		let identity = viewer["id"].as_str().unwrap().parse().unwrap();
		(
			source.receiver.deployment.to_string(),
			Bearer::Viewer(&source.remote, identity),
			String::new(),
		)
	} else {
		(
			source.receiver.to_string(),
			Bearer::Signed(&source.remote, source.node.clone()),
			viewer["subject"].as_str().unwrap().to_owned(),
		)
	};
	let response = peering::post_as(
		&app.client(),
		&receiver,
		bearer,
		&subject,
		"/federation/v0.1/scoped/graph",
		payload,
	)
	.await;
	(
		response.status_code(),
		response.json_value().unwrap_or(Value::Null),
	)
}

/// `local`'s Tenant maps `subject` of the Node `source` (a Tenant of `other`'s
/// Deployment) to a new credential of its own `subject`.
#[allow(dead_code)]
pub async fn map_subject(
	local: (&Federation, &TestApplication, &str),
	other: (&Federation, &str),
	subject: &str,
) {
	let (f, app, tenant) = local;
	let (other, other_tenant) = other;
	let (status, issued) = request(
		app,
		&f.config.api_token,
		"POST",
		&format!("/api/authorization/{tenant}/credentials"),
		json!({"subject":subject}),
	)
	.await;
	assert_eq!(status, 200, "{issued}");
	let (status, mapping) = request(
		app,
		&f.config.api_token,
		"POST",
		&format!("/api/authorization/{tenant}/peer-mappings"),
		json!({
			"node":default_node(&f.store, tenant).await.node,
			"source_node":default_node(&other.store, other_tenant).await,
			"source_subject":subject,"credential_id":issued["credential"]["id"],
			"enabled":true,"expected_revision":0,
		}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
}

/// Both fixture Deployments register the other's default Node, so each accepts
/// and addresses the other. Both serve their routes.
#[allow(dead_code)]
pub async fn register_pair(
	a: (&Federation, &TestApplication, &str),
	b: (&Federation, &TestApplication, &str),
) {
	register_default_node(a, (b.0, b.2)).await;
	register_default_node(b, (a.0, a.2)).await;
}

/// A request to `receiver`'s routes from `from`'s default Node of
/// `from_tenant`, as `subject`, signed with `from`'s Deployment Key. `path` is
/// the full path of the route, which starts with `/federation/v0.1`.
#[allow(dead_code)]
pub async fn peer_request(
	app: &TestApplication,
	from: (&Federation, &str),
	receiver: &aidash_domain::nodes::NodeAddress,
	subject: &str,
	path: &str,
	input: Value,
) -> (u16, Value) {
	let (f, tenant) = from;
	let node = default_node(&f.store, tenant).await;
	let body = input.to_string();
	let target = path
		.strip_prefix("/federation/v0.1")
		.expect("a federation route");
	let token = peering::token(
		f.config
			.deployment_key
			.as_deref()
			.expect("the fixture Deployment holds a key"),
		f.config.deployment.as_str(),
		&node.to_string(),
		&receiver.to_string(),
		subject,
		("POST", target, body.as_bytes()),
	)
	.await;
	let client = app.client();
	let mut request = client
		.request(http::Method::POST, path)
		.body(bytes::Bytes::copy_from_slice(body.as_bytes()))
		.header(http::header::CONTENT_TYPE, "application/json");
	for (name, value) in peering::peer_headers(&token) {
		request = request.header(name, value);
	}
	let response = request.send().await.unwrap();
	(
		response.status_code(),
		serde_json::from_slice(response.body()).unwrap_or(Value::Null),
	)
}

fn json_response(method: &str, path: &str, status: u16, body: &[u8]) -> (u16, Value) {
	if method == "HEAD" || matches!(status, 204 | 205 | 304) {
		assert!(body.is_empty(), "bodyless response for {method} {path}");
		(status, Value::Null)
	} else {
		(
			status,
			serde_json::from_slice(body).expect("JSON endpoint response"),
		)
	}
}

#[allow(dead_code)] // Explicit teardown Acts close resources before lifecycle assertions.
pub async fn cleanup(f: Federation, url: &str, database: &str) {
	f.store.control_pool.close().await;
	f.store.pool.close().await;
	cleanup_database(url, database).await;
}

#[allow(dead_code)] // The persistence-only suite shares the same database lifecycle.
pub async fn cleanup_database(_url: &str, database: &str) {
	assert!(database.starts_with("execution_"));
	assert!(
		database
			.bytes()
			.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
	);
	// TestEnvironment owns the entire disposable postmaster. Retain isolated
	// databases until its container is dropped: DROP DATABASE broadcasts a
	// process barrier even to other databases and can block behind concurrent
	// PGroonga index initialization. Callers close their process-owned pools;
	// container teardown removes every fixture database together.
}

#[allow(dead_code)]
pub fn policy(deployment: &str) -> Value {
	let agent = qualified_agent(
		&tenant_roots::fixture_node(deployment, "acme"),
		"research",
		"1.0.0",
	);
	// Peer flows still name the executing Node by its Deployment id (#241).
	let peer_agent = aidash_domain::agent_subject(deployment, "research", "1.0.0");
	json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},agent:{"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")},peer_agent:{"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")}},
        "policies":[{"id":"approved-work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
            // A wildcard never grants calling a Tenant's tool without approval.
            {"id":"call-unapproved","effect":"allow","subjects":{"any":true},"actions":["tool.call_unapproved"],"resources":{"kinds":["tool"]}}]})
}

#[allow(dead_code)]
pub async fn bootstrap(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
) -> (Value, String, Uuid) {
	bootstrap_with_context(f, app, endpoint, false).await
}
#[allow(dead_code)]
pub async fn bootstrap_with_context(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
	semantic: bool,
) -> (Value, String, Uuid) {
	bootstrap_with_resources(f, app, endpoint, semantic, false).await
}
#[allow(dead_code)]
pub async fn bootstrap_with_area(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
) -> (Value, String, Uuid) {
	bootstrap_with_resources(f, app, endpoint, false, true).await
}
async fn bootstrap_with_resources(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
	semantic: bool,
	files: bool,
) -> (Value, String, Uuid) {
	bootstrap_with_policy(
		f,
		app,
		endpoint,
		semantic,
		files,
		false,
		policy(f.config.deployment.as_str()),
	)
	.await
}
/// Like [`bootstrap`], under a policy that grants no direct permission to call a
/// Tenant's tool, so every such call waits for a Tool call approval.
#[allow(dead_code)]
pub async fn bootstrap_requiring_tool_call_approval(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
) -> (Value, String, Uuid) {
	let mut policy = policy(f.config.deployment.as_str());
	require_tool_call_approval(&mut policy);
	bootstrap_with_policy(f, app, endpoint, false, false, false, policy).await
}
/// Like [`bootstrap_requiring_tool_call_approval`], and the Agent also binds
/// `http-read` as `plugin_1`: its version declares the one operation read-only,
/// and the endpoint's `/read` route serves it. `plugin_0` still asks.
#[allow(dead_code)]
pub async fn bootstrap_with_read_only_tool(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
) -> (Value, String, Uuid) {
	let mut policy = policy(f.config.deployment.as_str());
	require_tool_call_approval(&mut policy);
	bootstrap_with_policy(f, app, endpoint, false, false, true, policy).await
}
/// Narrow `approved-work` so that no subject may call a Tenant's tool without a
/// Tool call approval: agents keep every action, and users keep what members
/// have, including approving the tool calls of their own Runs (ADR 0075).
#[allow(dead_code)]
pub fn require_tool_call_approval(policy: &mut Value) {
	let resources = policy["policies"][0]["resources"].clone();
	policy["policies"][0]["subjects"] = json!({"kinds":["agent"]});
	let policies = policy["policies"].as_array_mut().unwrap();
	policies.retain(|policy| policy["id"] != "call-unapproved");
	policies.push(json!({"id":"member-work","effect":"allow","subjects":{"kinds":["user"]},"actions":aidash_domain::identity::tenancy::MEMBER_ACTIONS,"resources":resources}));
	policies.push(json!({"id":"member-tool-call-approval","effect":"allow","subjects":{"kinds":["user"]},"actions":["capability.approve"],"resources":{"kinds":["tool", "sampling"]}}));
}
/// The Tool call approval the Run waits for, with the call it asks about.
#[allow(dead_code)]
pub fn awaited_tool_call(
	run: &aidash_server::domain::Run,
) -> Option<(Uuid, aidash_domain::provider::ToolCall)> {
	use aidash_domain::{RunState, WaitingState, run_state::ApprovalRef};
	let RunState::Waiting(waiting) = &run.state else {
		return None;
	};
	let WaitingState::Approval {
		approval: ApprovalRef::Capability { approval_id },
		resume,
	} = waiting.as_ref()
	else {
		return None;
	};
	Some((
		*approval_id,
		resume.response.tool_calls[resume.cursor].clone(),
	))
}
/// Decide the pending Tool call approval `approval` as the approver `token`
/// names, through the same list and decision API the dashboard uses.
/// `choice` is `allow_once`, `allow_run` or `deny`; an `allow_run` lasts ten minutes.
#[allow(dead_code)]
pub async fn decide_tool_call(
	app: &TestApplication,
	token: &str,
	approval: Uuid,
	choice: &str,
) -> (u16, Value) {
	let (status, inbox) = request(
		app,
		token,
		"GET",
		"/api/capabilities/approvals?state=pending&role=approver",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{inbox}");
	let card = inbox["items"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["id"] == json!(approval))
		.unwrap_or_else(|| panic!("approval {approval} is not pending for this approver: {inbox}"));
	assert_eq!(card["kind"], "tool_call", "{card}");
	let mut decision = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":card["revision"],"choice":choice});
	if choice == "allow_run" {
		decision["targets"] = card["targets"].clone();
		decision["expires_at"] = json!(chrono::Utc::now() + chrono::Duration::minutes(10));
	}
	request(
		app,
		token,
		"POST",
		&format!("/api/capabilities/approvals/{approval}/decide"),
		decision,
	)
	.await
}
async fn bootstrap_with_policy(
	f: &Federation,
	app: &TestApplication,
	endpoint: &str,
	semantic: bool,
	files: bool,
	read_only_tool: bool,
	policy: Value,
) -> (Value, String, Uuid) {
	let deployment_administrator = &f.config.api_token;
	tenant_root(f, "acme").await;
	// A fixture that already wrote the Tenant's policy, such as a federated
	// pair, is replaced at the revision it holds.
	let revision: i64 =
		sqlx::query_scalar("SELECT revision FROM authorization_bundles WHERE tenant = 'acme'")
			.fetch_optional(f.store.pool.fixture_driver())
			.await
			.unwrap()
			.unwrap_or(0);
	let (status, snapshot) = request(
		app,
		deployment_administrator,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":revision,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "initial policy: {snapshot}");
	let mut agent_bindings = vec![
		json!({"kind":"tool","target":{"registry_deployment":f.config.deployment,"id":"http","version":"1.0.0"},"alias":"plugin_0","narrow":{}}),
	];
	if read_only_tool {
		agent_bindings.push(json!({"kind":"tool","target":{"registry_deployment":f.config.deployment,"id":"http-read","version":"1.0.0"},"alias":"plugin_1","narrow":{}}));
	}
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let approve = async |id: &str| {
		authorization
			.set_catalog(
				"acme",
				&tenant_roots::fixture_node_name("acme"),
				&EntityRef {
					id: id.into(),
					version: "1.0.0".into(),
				},
				0,
				true,
				"deployment_administrator",
			)
			.await
			.unwrap();
	};
	if semantic {
		let memory = json!({"id":"semantic-memory","version":"1.0.0","kind":"memory","name":{"en":"Semantic Memory"},"description":{"en":"Explicit retrieval opt-in"},"schema":{},"config":{"schema_version":1,"source":{"adapter":"semantic_memory"}}});
		f.registry
			.register(&tenant, serde_json::from_value(memory).unwrap())
			.await
			.unwrap();
		approve("semantic-memory").await;
		agent_bindings.push(json!({"kind":"memory","target":{"registry_deployment":f.config.deployment,"id":"semantic-memory","version":"1.0.0"},"narrow":{}}));
	}
	// The model fixture is a Keyless endpoint on loopback (ADR 0036). An
	// unreachable fixture address stays permitted so it keeps failing to connect.
	f.store.keyless.permit_origin_of(endpoint).unwrap();
	let mut definitions = vec![
		(
			"model",
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"tool",
			"http",
			json!({"registry_deployment":f.config.deployment,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","transport":{"transport":"http","endpoint":format!("{endpoint}/effect"),"replay":"idempotent"}}),
		),
		(
			"agent",
			"research",
			json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Test approved work","bindings":agent_bindings,"remove_default":if files {json!([])} else {json!(["file_search","file_read"])} }),
		),
	];
	if read_only_tool {
		definitions.insert(
			2,
			(
				"tool",
				"http-read",
				json!({"registry_deployment":f.config.deployment,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_1","tier":"integration","transport":{"transport":"http","endpoint":format!("{endpoint}/read"),"replay":"read_only"}}),
			),
		);
	}
	for (kind, id, config) in definitions {
		let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":config});
		f.registry
			.register(&tenant, serde_json::from_value(entry).unwrap())
			.await
			.unwrap();
		approve(id).await;
	}
	let (status, credential) = request(
		app,
		deployment_administrator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200);
	let token = credential["token"].as_str().unwrap().to_owned();
	let (status, workspace) = request(
		app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"Approved task","goal":"Use exactly one tool"}),
	)
	.await;
	assert_eq!(status, 200);
	let (status, task) = request(
		app,
		&token,
		"POST",
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Research","description":"Use approved tools"}),
	)
	.await;
	assert_eq!(status, 200);
	(
		policy,
		token,
		Uuid::parse_str(task["id"].as_str().unwrap()).unwrap(),
	)
}

/// Explicit native process settings for the fixture's isolated database.
#[allow(dead_code)]
pub fn native_process_environment(
	f: &Federation,
	database_url: &str,
	directory: &std::path::Path,
	worker_count: usize,
) -> Vec<(&'static str, String)> {
	let mut settings = settings_for(database_url);
	let database = reqwest::Url::parse(database_url).unwrap();
	settings
		.core
		.databases
		.get_mut("default")
		.unwrap()
		.options
		.extend(
			database
				.query_pairs()
				.map(|(key, value)| (key.to_string(), value.to_string())),
		);
	settings.deployment.id = f.config.deployment.to_string();
	settings.deployment.endpoint = f.config.endpoint.clone();
	settings.deployment.api_token = f.config.api_token.clone();
	settings.deployment.nats_url = f.config.nats_url.clone();
	settings.deployment.web_dir = f.config.web_dir.clone();
	settings.deployment.background_enabled = true;
	settings.deployment.worker_count = worker_count;
	settings.dashboard.oidc = f.config.oidc.clone();
	// A child process must reach the same Keyless fixtures (ADR 0036).
	settings.keyless_endpoints.permitted_private_origins = f.store.keyless.permitted_origins();
	peering::configure_process(&mut settings, f);
	let destination = directory.join(format!("settings-{}", Uuid::new_v4()));
	std::fs::create_dir_all(&destination).unwrap();
	std::fs::write(destination.join("base.toml"), process_settings(&settings)).unwrap();
	let listen = reqwest::Url::parse(&f.config.endpoint)
		.unwrap()
		.socket_addrs(|| None)
		.unwrap();
	assert_eq!(listen.len(), 1, "fixture must advertise one local listener");
	vec![
		// Multi-process fixtures need bounded executors even on high-core hosts.
		("TOKIO_WORKER_THREADS", "2".into()),
		(
			"REINHARDT_SETTINGS_DIR",
			destination.to_string_lossy().into_owned(),
		),
		("REINHARDT_ENV", "container".into()),
		("AIDASH_LISTEN", listen[0].to_string()),
	]
}
#[allow(dead_code)]
pub fn native_process_args(_f: &Federation, mode: &str) -> Vec<String> {
	vec![mode.to_owned()]
}

/// Every migration of the installed build: Community's, then the build's own.
pub async fn native_migrations() -> Vec<reinhardt::db::migrations::Migration> {
	let server = PathBuf::from(aidash_server::bootstrap::build::Build::COMMUNITY.manifest_dir);
	let mut migrations = Vec::new();
	for root in aidash_server::bootstrap::build::current().migration_roots(&server) {
		migrations.extend(
			FilesystemSource::new(root)
				.all_migrations()
				.await
				.expect("load the native migration graph"),
		);
	}
	migrations
}

/// A Store on the server role's URL, after migrating the same database as its owner.
#[allow(dead_code)]
pub async fn native_store(database_url: &str, node: &str) -> Store {
	let owner = sqlx::PgPool::connect(&postgres::with_login(database_url, postgres::OWNER))
		.await
		.unwrap();
	let connection = DatabaseConnection::new(Arc::new(PostgresBackend::new(owner.clone())));
	let migrations = native_migrations().await;
	postgres::apply_migrations(connection, &migrations).await;
	owner.close().await;
	let pool = sqlx::PgPool::connect(database_url).await.unwrap();
	Store::from_pool(
		pool,
		aidash_domain::nodes::DeploymentId::parse(node).unwrap(),
	)
	.await
	.unwrap()
}

#[path = "state.rs"]
mod state;
#[allow(unused_imports)] // Shared fixture exports vary by integration target.
pub use state::{context, pending, tool_call, tool_pending};

use environment::EnvironmentFuture;
use futures_util::{
	FutureExt,
	future::{BoxFuture, Shared},
};
use rstest::fixture;

pub type DatabaseFuture = Shared<BoxFuture<'static, Arc<ExecutionDatabase>>>;
pub type RuntimeFuture = Shared<BoxFuture<'static, RuntimeFixture>>;

pub struct ExecutionDatabase {
	/// The server role's pool, which row-level security binds.
	pub pool: sqlx::PgPool,
	/// The server role's URL.
	pub url: String,
	/// The owner's URL, for migrations and fixture-only DDL.
	#[allow(dead_code)]
	pub owner_url: String,
	pub schema: String,
	environment: Arc<TestEnvironment>,
}

#[derive(Clone)]
#[allow(dead_code)] // Shared integration binaries independently use each runtime value.
pub struct RuntimeFixture {
	pub federation: Federation,
	pub url: String,
	pub schema: String,
	/// A native child process's port, retained through asynchronous fixture setup.
	pub process_listener: Option<Arc<std::sync::Mutex<Option<std::net::TcpListener>>>>,
	_database: Arc<ExecutionDatabase>,
}

impl RuntimeFixture {
	/// The disposable services that also bound this runtime's fixture stages.
	pub fn environment(&self) -> Arc<TestEnvironment> {
		self._database.environment.clone()
	}

	/// Project values while this fixture retains its disposable infrastructure.
	#[allow(dead_code)] // Database-only and process-only targets do not unpack HTTP runtime state.
	pub fn parts(&self) -> (Federation, String, String) {
		(
			self.federation.clone(),
			self.url.clone(),
			self.schema.clone(),
		)
	}
}

#[fixture]
pub fn execution_database(
	test_environment: EnvironmentFuture,
	#[default("execution")] prefix: &str,
	#[default(12)] max_connections: u32,
) -> DatabaseFuture {
	let prefix = prefix.to_owned();
	async move {
		let environment = test_environment.await;

		let database = format!("{prefix}_{}", Uuid::new_v4().simple());
		// The preserved baseline names public explicitly. Isolate databases, not
		// search paths. TestEnvironment owns the container and every database, so
		// unwinding also removes fixtures that never reach explicit cleanup.
		// Each stage names the database, which the PostgreSQL snapshot of a
		// hang lists for every session connected to it (#320).
		let logins = environment
			.bounded_setup(
				format!("create the fixture database {database}"),
				postgres::prepare_database(
					&environment.database_url,
					&database,
					postgres::PgroongaRecovery::Off,
				),
			)
			.await;
		// Migrations run as the owner; everything else uses the server role.
		environment
			.bounded_setup(format!("migrate the fixture database {database}"), async {
				let owner = PgPoolOptions::new()
					.max_connections(2)
					.connect(&logins.owner_url)
					.await
					.unwrap();
				let migrations = native_migrations().await;
				postgres::apply_migrations(
					DatabaseConnection::new(Arc::new(PostgresBackend::new(owner.clone()))),
					&migrations,
				)
				.await;
				owner.close().await;
			})
			.await;
		let url = logins.server_url;
		let options = PgConnectOptions::from_str(&url)
			.unwrap()
			.application_name(&database);
		let pool = environment
			.bounded_setup(
				format!("connect the server pool to {database}"),
				PgPoolOptions::new()
					.max_connections(max_connections)
					.connect_with(options),
			)
			.await
			.unwrap();
		Arc::new(ExecutionDatabase {
			pool,
			url,
			owner_url: logins.owner_url,
			schema: database,
			environment,
		})
	}
	.boxed()
	.shared()
}

#[fixture]
pub fn runtime(execution_database: DatabaseFuture, http_client: reqwest::Client) -> RuntimeFuture {
	async move {
		let database = execution_database.await;
		// The deterministic research fixture provider never leaves the Node.
		aidash_server::research::configure(
			&serde_json::from_value(json!({"page_reading": false, "fixture": true, "brave": null}))
				.unwrap(),
			&reinhardt::conf::settings::profile::Profile::Development,
		)
		.unwrap();

		let store = database
			.environment
			.bounded_setup(
				format!("open the Store on {}", database.schema),
				Store::from_pool(
					database.pool.clone(),
					aidash_domain::nodes::DeploymentId::parse("aidash://execution-test").unwrap(),
				),
			)
			.await
			.unwrap();
		// Fixtures permit their origins on the Store after startup.
		let registry = Registry::new(database.pool.clone(), &store.deployment)
			.unwrap()
			.with_keyless_origins(store.keyless.clone());
		let mut federation = Federation {
			sandbox: Default::default(),
			store,
			registry,
			config: Config {
				deployment: aidash_domain::nodes::DeploymentId::parse("aidash://execution-test")
					.unwrap(),
				endpoint: "http://localhost:8080".into(),
				database_url: database.url.clone(),
				nats_url: database.environment.nats_url.clone(),
				api_token: "deployment-administrator-execution-fixture".into(),
				web_dir: "web/dist".into(),
				lease_seconds: 30,
				tenancy: aidash_domain::identity::tenancy::TenancyRules::of(
					aidash_domain::offering::Offering::SelfHosted,
				),
				demo_data: false,
				oidc: None,
				sign_in: None,
				marketplace: aidash_server::config::marketplace::MarketplaceSettings {
					endpoint: None,
					connections: Vec::new(),
					poll_seconds: 60,
				},
				common_auth: None,
				common_issuers: None,
				federation: true,
				deployment_key: None,
				tier_limits: std::sync::Arc::new(aidash_application::ports::nodes::NoTierLimits),
				preview: None,
				dashboard_origin: "http://localhost:3000".into(),
			},
			client: http_client,
			notify: Arc::new(tokio::sync::Notify::new()),
		};
		// Every fixture Deployment holds its own Deployment Key (#241).
		peering::give_key(&mut federation);
		RuntimeFixture {
			federation,
			url: database.url.clone(),
			schema: database.schema.clone(),
			process_listener: None,
			_database: database,
		}
	}
	.boxed()
	.shared()
}
