#![allow(clippy::disallowed_methods)]
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "../../execution/tests/support/endpoint.rs"]
mod support;
#[path = "../../execution/tests/support/upstream.rs"]
mod upstream;

use aidash_application::{
	ports::registry::{AgentPluginSource, FetchedPlugin, PluginFetch, PluginImportRequest},
	registry::agent_plugin_import::AgentPluginImport,
	tenancy::TenancyScope,
};
use aidash_domain::agent_plugin::{self, AgentPluginRef, PLUGIN_SCHEMA, PluginFile};
use aidash_domain::identity::tenancy::{ADMINISTRATORS, MEMBERS, initial_policy};
use aidash_domain::policy::{Policy, ResourceSelector, Subject, SubjectKind, SubjectSelector};
use aidash_domain::tenancy::TenantName;
use reinhardt::test::APIClient;
use reinhardt::test::fixtures::api_client_from_url;
use rstest::rstest;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use support::{EndpointFixture, assert_json, endpoint, tenant_root};
use uuid::Uuid;

const NODE: &str = "aidash://endpoint-test";

async fn tenant(fixture: &EndpointFixture, name: &str) {
	tenant_root(&fixture.runtime, name).await;
	let mut bundle = initial_policy(name, false).unwrap();
	for (subject, groups) in [
		("owner", vec![ADMINISTRATORS]),
		("member", vec![MEMBERS]),
		("reader", vec![]),
	] {
		bundle.subjects.insert(
			subject.into(),
			Subject {
				kind: SubjectKind::User,
				groups: groups.into_iter().map(str::to_owned).collect(),
				roles: BTreeSet::new(),
				attributes: json!({}),
				enabled: true,
				delegated_by: None,
			},
		);
	}
	bundle.policies.push(Policy {
		id: "reader".into(),
		effect: aidash_domain::policy::Effect::Allow,
		subjects: SubjectSelector {
			ids: ["reader".to_owned()].into(),
			..Default::default()
		},
		actions: ["registry.read".to_owned()].into(),
		resources: ResourceSelector {
			kinds: ["*".to_owned()].into(),
			ids: BTreeSet::new(),
		},
		condition: None,
	});
	bundle.validate().unwrap();
	assert_json(
		fixture
			.deployment_administrator
			.post(
				&format!("/api/authorization/{name}"),
				&json!({"expected_revision":0,"bundle":bundle}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
}

async fn credential(fixture: &EndpointFixture, tenant: &str, subject: &str) -> Arc<APIClient> {
	let issued = assert_json(
		fixture
			.deployment_administrator
			.post(
				&format!("/api/authorization/{tenant}/credentials"),
				&json!({"subject":subject}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let client = api_client_from_url(&fixture.server.url);
	client
		.set_header(
			"Authorization",
			&format!("Bearer {}", issued["token"].as_str().unwrap()),
		)
		.await
		.unwrap();
	Arc::new(client)
}

fn model(id: &str) -> Value {
	json!({"id":id,"version":"1.0.0","kind":"model","name":{"en":id},"description":{"en":"Acceptance"},"schema":{},
		"config":{"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:9/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}})
}
fn agent(id: &str, version: &str, model: &str) -> Value {
	json!({"id":id,"version":version,"kind":"agent","name":{"en":id},"description":{"en":"Acceptance"},"schema":{},
		"config":{"schema_version":1,"model":{"id":model,"version":"1.0.0"},"instructions":"Help","bindings":[],"remove_default":[]}})
}
fn approval(id: &str, version: &str, enabled: bool) -> Value {
	json!({"entry":{"id":id,"version":version},"enabled":enabled,"expected_revision":0})
}

async fn register(client: &APIClient, entry: &Value) -> (u16, Value) {
	let response = client.post("/api/registry", entry, "json").await.unwrap();
	(
		response.status_code(),
		response.json_value().unwrap_or(Value::Null),
	)
}
async fn approve(client: &APIClient, tenant: &str, approvals: Value) -> (u16, Value) {
	let response = client
		.post(
			&format!("/api/authorization/{tenant}/catalog/approvals"),
			&json!({"approvals":approvals}),
			"json",
		)
		.await
		.unwrap();
	(
		response.status_code(),
		response.json_value().unwrap_or(Value::Null),
	)
}
async fn policy(fixture: &EndpointFixture, tenant: &str) -> Value {
	assert_json(
		fixture
			.deployment_administrator
			.get(&format!("/api/authorization/{tenant}"))
			.await
			.unwrap(),
		200,
	)
}
async fn count(fixture: &EndpointFixture, sql: &str, tenant: &str) -> i64 {
	sqlx::query_scalar::<_, i64>(sql)
		.bind(tenant)
		.fetch_one(fixture.runtime.store.pool.fixture_driver())
		.await
		.unwrap()
}

#[rstest]
#[tokio::test]
async fn a_tenant_registers_approves_and_runs_its_own_agent_without_a_deployment_administrator(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
	// Act: two versions of one agent and its model, registered by the Tenant.
	for entry in [
		model("model"),
		agent("helper", "1.0.0", "model"),
		agent("helper", "1.1.0", "model"),
	] {
		let (status, body) = register(&owner, &entry).await;
		assert_eq!(status, 200, "{body}");
	}
	let (refused, message) =
		approve(&owner, "acme", json!([approval("helper", "1.1.0", true)])).await;
	let (approved, bindings) = approve(
		&owner,
		"acme",
		json!([
			approval("helper", "1.1.0", true),
			approval("model", "1.0.0", true)
		]),
	)
	.await;
	let response = owner
		.post(
			"/api/conversations",
			&json!({"title":"Mine","goal":"Use my agent","target":{"id":"helper","version":"1.1.0"},"target_kind":"agent"}),
			"json",
		)
		.await
		.unwrap();
	// Assert
	assert_eq!(refused, 400, "{message}");
	assert!(
		message
			.to_string()
			.contains("CATALOG_DEPENDENCY_UNAPPROVED"),
		"{message}"
	);
	assert_eq!(approved, 200, "{bindings}");
	let subject = &policy(&fixture, "acme").await["bundle"]["subjects"]
		[format!("{NODE}/agents/helper@1.1.0")];
	assert_eq!(subject["kind"], "agent");
	assert_eq!(subject["groups"], json!([MEMBERS]));
	assert!(
		subject.get("delegated_by").is_none_or(Value::is_null),
		"{subject}"
	);
	assert_eq!(response.status_code(), 200, "{}", response.text());
	let versions = assert_json(owner.get("/api/registry/versions").await.unwrap(), 200);
	let unapproved = versions["versions"]
		.as_array()
		.unwrap()
		.iter()
		.find(|version| {
			version["entry"]["id"] == "helper" && version["entry"]["version"] == "1.0.0"
		})
		.unwrap();
	assert_eq!(unapproved["approval"], Value::Null);
	assert_eq!(unapproved["needs_approval"], true);
}

#[rstest]
#[tokio::test]
async fn another_tenant_cannot_read_reference_or_approve_the_definition(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	tenant(&fixture, "globex").await;
	let acme = credential(&fixture, "acme", "owner").await;
	let globex = credential(&fixture, "globex", "owner").await;
	assert_eq!(register(&acme, &model("model")).await.0, 200);
	assert_eq!(
		approve(&acme, "acme", json!([approval("model", "1.0.0", true)]))
			.await
			.0,
		200
	);
	// Act
	let read = globex
		.get("/api/registry/model/1.0.0")
		.await
		.unwrap()
		.status_code();
	let (reference, _) = register(&globex, &agent("helper", "1.0.0", "model")).await;
	let (approval_status, _) =
		approve(&globex, "globex", json!([approval("model", "1.0.0", true)])).await;
	let cross = globex
		.post(
			"/api/authorization/acme/catalog",
			&approval("model", "1.0.0", false),
			"json",
		)
		.await
		.unwrap()
		.status_code();
	let listed = assert_json(globex.get("/api/registry/versions").await.unwrap(), 200);
	// Assert
	assert_ne!(read, 200);
	assert_ne!(reference, 200);
	assert_ne!(approval_status, 200);
	assert_eq!(cross, 403);
	assert!(
		listed["versions"]
			.as_array()
			.unwrap()
			.iter()
			.all(|version| version["entry"]["id"] != "model"),
		"{listed}"
	);
	assert_eq!(
		count(
			&fixture,
			"SELECT count(*) FROM registry WHERE tenant = $1 AND id = 'model'",
			"globex"
		)
		.await,
		0
	);
	assert_eq!(register(&globex, &model("model")).await.0, 200);
}

#[rstest]
#[tokio::test]
async fn idempotency_keys_belong_to_a_tenant_and_a_replay_decides_again(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	tenant(&fixture, "globex").await;
	let acme = credential(&fixture, "acme", "owner").await;
	let globex = credential(&fixture, "globex", "owner").await;
	let key = Uuid::new_v4().to_string();
	let unnamed = json!({"id":"","version":"1.0.0","kind":"skill","name":{"en":"Skill"},"description":{"en":"Acceptance"},"schema":{},"config":{"instructions":"Follow"}});
	for client in [&acme, &globex] {
		client.set_header("Idempotency-Key", &key).await.unwrap();
	}
	// Act
	let (first, created) = register(&acme, &unnamed).await;
	let (other_tenant, elsewhere) = register(&globex, &unnamed).await;
	let (replayed, again) = register(&acme, &unnamed).await;
	let mut changed = unnamed.clone();
	changed["config"]["instructions"] = json!("Changed");
	let (conflict, _) = register(&acme, &changed).await;
	let reader = credential(&fixture, "acme", "reader").await;
	reader.set_header("Idempotency-Key", &key).await.unwrap();
	let (denied, _) = register(&reader, &unnamed).await;
	// Assert
	assert_eq!(first, 200, "{created}");
	assert_eq!(other_tenant, 200, "{elsewhere}");
	assert_eq!(replayed, 200);
	assert_eq!(again["id"], created["id"]);
	assert_ne!(elsewhere["id"], Value::Null);
	assert_eq!(conflict, 409);
	assert_eq!(denied, 403);
}

#[rstest]
#[tokio::test]
async fn a_denied_write_leaves_no_definition_request_approval_or_event(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let reader = credential(&fixture, "acme", "reader").await;
	let member = credential(&fixture, "acme", "member").await;
	reader
		.set_header("Idempotency-Key", &Uuid::new_v4().to_string())
		.await
		.unwrap();
	// Act
	let (registration, _) = register(&reader, &model("model")).await;
	let (registered, _) = register(&member, &model("model")).await;
	let (approval_status, _) =
		approve(&member, "acme", json!([approval("model", "1.0.0", true)])).await;
	let deployment = fixture
		.deployment_administrator
		.post("/api/registry", &model("other"), "json")
		.await
		.unwrap()
		.status_code();
	// Assert
	assert_eq!(registration, 403);
	assert_eq!(registered, 200);
	assert_eq!(approval_status, 403);
	assert_eq!(deployment, 403);
	assert_eq!(
		count(
			&fixture,
			"SELECT count(*) FROM registry_requests WHERE tenant = $1",
			"acme"
		)
		.await,
		0
	);
	assert_eq!(
		count(
			&fixture,
			"SELECT count(*) FROM authorization_catalog WHERE tenant = $1",
			"acme"
		)
		.await,
		0
	);
	assert_eq!(
		count(
			&fixture,
			"SELECT count(*) FROM registry WHERE tenant = $1 AND id = 'other'",
			"acme"
		)
		.await,
		0
	);
	assert!(
		count(
			&fixture,
			"SELECT count(*) FROM authorization_decisions WHERE tenant = $1 AND NOT (decision->>'allowed')::boolean",
			"acme",
		)
		.await
			>= 2
	);
}

#[rstest]
#[tokio::test]
async fn a_sandboxed_built_in_is_usable_only_after_the_tenant_approves_it(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
	let versions = assert_json(owner.get("/api/registry/versions").await.unwrap(), 200);
	let bundle = versions["versions"]
		.as_array()
		.unwrap()
		.iter()
		.find(|version| version["entry"]["id"] == "aidash.bundle.outbound_get")
		.unwrap_or_else(|| panic!("the Deployment offers outbound requests: {versions}"))
		.clone();
	assert_eq!(bundle["needs_approval"], true);
	assert_eq!(bundle["approval"], Value::Null);
	// Act
	let (alone, refusal) = approve(
		&owner,
		"acme",
		json!([approval("aidash.bundle.outbound_get", "1.0.0", true)]),
	)
	.await;
	let (closure, _) = approve(
		&owner,
		"acme",
		json!([
			approval("aidash.bundle.outbound_get", "1.0.0", true),
			approval("aidash.outbound_get", "1.0.0", true)
		]),
	)
	.await;
	// Assert
	assert_eq!(alone, 400, "{refusal}");
	assert_eq!(closure, 200);
}

#[rstest]
#[tokio::test]
async fn agent_groups_need_group_authority_and_approval_never_restores_a_disabled_agent(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange: a member holding `catalog.approve` but not `subject.group.update`.
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
	assert_eq!(register(&owner, &model("model")).await.0, 200);
	assert_eq!(
		register(&owner, &agent("helper", "1.0.0", "model")).await.0,
		200
	);
	assert_eq!(
		approve(&owner, "acme", json!([approval("model", "1.0.0", true)]))
			.await
			.0,
		200
	);
	let mut document = policy(&fixture, "acme").await;
	let revision = document["revision"].as_i64().unwrap();
	document["bundle"]["policies"]
		.as_array_mut()
		.unwrap()
		.push(json!({
			"id":"approver","effect":"allow","subjects":{"ids":["member"]},
			"actions":["catalog.approve"],"resources":{"kinds":["*"]}
		}));
	assert_json(
		fixture
			.deployment_administrator
			.post(
				"/api/authorization/acme",
				&json!({"expected_revision":revision,"bundle":document["bundle"]}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let member = credential(&fixture, "acme", "member").await;
	let mut elevated = approval("helper", "1.0.0", true);
	elevated["groups"] = json!([ADMINISTRATORS]);
	// Act
	let (named, _) = approve(&member, "acme", json!([elevated])).await;
	let (defaulted, _) = approve(&member, "acme", json!([approval("helper", "1.0.0", true)])).await;
	let subject = format!("{NODE}/agents/helper@1.0.0");
	let mut document = policy(&fixture, "acme").await;
	let revision = document["revision"].as_i64().unwrap();
	document["bundle"]["subjects"][&subject]["enabled"] = json!(false);
	assert_json(
		fixture
			.deployment_administrator
			.post(
				"/api/authorization/acme",
				&json!({"expected_revision":revision,"bundle":document["bundle"]}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let withdrawn = owner
		.post(
			"/api/authorization/acme/catalog",
			&json!({"entry":{"id":"helper","version":"1.0.0"},"enabled":false,"expected_revision":1}),
			"json",
		)
		.await
		.unwrap()
		.status_code();
	let restored = owner
		.post(
			"/api/authorization/acme/catalog",
			&json!({"entry":{"id":"helper","version":"1.0.0"},"enabled":true,"expected_revision":2}),
			"json",
		)
		.await
		.unwrap()
		.status_code();
	// Assert
	assert_eq!(named, 403);
	assert_eq!(defaulted, 200);
	assert_eq!(withdrawn, 200);
	assert_eq!(restored, 200);
	let after = &policy(&fixture, "acme").await["bundle"]["subjects"][&subject];
	assert_eq!(after["groups"], json!([MEMBERS]));
	assert_eq!(after["enabled"], false);
}

#[rstest]
#[tokio::test]
async fn the_versions_list_decides_each_version_and_shows_agent_subjects_to_approvers(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange: the reader may read the Registry except one model.
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
	for entry in [
		model("open"),
		model("secret"),
		agent("helper", "1.0.0", "open"),
	] {
		assert_eq!(register(&owner, &entry).await.0, 200);
	}
	assert_eq!(
		approve(
			&owner,
			"acme",
			json!([
				approval("helper", "1.0.0", true),
				approval("open", "1.0.0", true)
			])
		)
		.await
		.0,
		200
	);
	let mut document = policy(&fixture, "acme").await;
	let revision = document["revision"].as_i64().unwrap();
	document["bundle"]["policies"]
		.as_array_mut()
		.unwrap()
		.push(json!({
			"id":"hide-secret","effect":"deny","subjects":{"ids":["reader"]},
			"actions":["registry.read"],"resources":{"kinds":["model"],"ids":["secret"]}
		}));
	document["bundle"]["policies"]
		.as_array_mut()
		.unwrap()
		.push(json!({
			"id":"approve-registry","effect":"allow","subjects":{"ids":["member"]},
			"actions":["catalog.approve"],"resources":{"kinds":["registry"]}
		}));
	assert_json(
		fixture
			.deployment_administrator
			.post(
				"/api/authorization/acme",
				&json!({"expected_revision":revision,"bundle":document["bundle"]}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let reader = credential(&fixture, "acme", "reader").await;
	let member = credential(&fixture, "acme", "member").await;
	// Act
	let read = assert_json(reader.get("/api/registry/versions").await.unwrap(), 200);
	let approving = assert_json(member.get("/api/registry/versions").await.unwrap(), 200);
	let administered = assert_json(owner.get("/api/registry/versions").await.unwrap(), 200);
	// Assert
	let find = |page: &Value, id: &str| {
		page["versions"]
			.as_array()
			.unwrap()
			.iter()
			.find(|version| version["entry"]["id"] == id)
			.cloned()
	};
	assert!(find(&read, "open").is_some(), "{read}");
	assert!(find(&read, "secret").is_none(), "{read}");
	assert_eq!(find(&read, "helper").unwrap()["subject"], Value::Null);
	assert_eq!(
		find(&approving, "helper").unwrap()["subject"]["groups"],
		json!([MEMBERS])
	);
	assert!(find(&administered, "secret").is_some());
	assert_eq!(
		find(&administered, "helper").unwrap()["subject"]["groups"],
		json!([MEMBERS])
	);
}

/// An MCP server over streamable HTTP that lists one tool per kind of
/// annotation: a read-only hint, a write hint and none. It counts the requests
/// it receives.
async fn mcp_server(
	requests: Arc<std::sync::atomic::AtomicUsize>,
) -> reinhardt::test::fixtures::server::TestServerGuard {
	let router = reinhardt::test::stub::StubRouter::new()
		.route(
			"/mcp",
			http::Method::POST,
			upstream::reply(move |request: reinhardt::Request| {
				requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
				async move {
					let message = request.json::<Value>().unwrap();
					let result = match message["method"].as_str().unwrap() {
						"initialize" => json!({
							"protocolVersion": message["params"]["protocolVersion"],
							"capabilities": {"tools": {}},
							"serverInfo": {"name": "fixture", "version": "1.0.0"}
						}),
						"tools/list" => json!({"tools": [
							{"name":"lookup","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},
							{"name":"write","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":false}},
							{"name":"plain","inputSchema":{"type":"object"}}
						]}),
						_ => return reinhardt::Response::new(reinhardt::StatusCode::ACCEPTED),
					};
					reinhardt::Response::ok()
						.with_json(&json!({"jsonrpc":"2.0","id":message["id"],"result":result}))
						.unwrap()
				}
			}),
		)
		.into_server_router();
	upstream::upstream(Arc::new(router)).await
}

#[rstest]
#[tokio::test]
async fn an_mcp_tool_hint_needs_registry_create_and_stores_nothing(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange: the owner may register definitions, the reader only reads them.
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
	let reader = credential(&fixture, "acme", "reader").await;
	let requests = Arc::<std::sync::atomic::AtomicUsize>::default();
	let server = mcp_server(requests.clone()).await;
	let mcp = format!("{}/mcp", server.url);
	let unreachable_endpoint = "http://127.0.0.1:1/mcp";
	// The Deployment permits these private origins; nothing else is reachable.
	for endpoint in [&mcp, unreachable_endpoint] {
		fixture
			.runtime
			.store
			.keyless
			.permit_origin_of(endpoint)
			.unwrap();
	}
	let hint = |client: Arc<APIClient>, endpoint: String, tool: &'static str| async move {
		client
			.post(
				"/api/registry/mcp-tool-hint",
				&json!({"endpoint": endpoint, "tool_name": tool}),
				"json",
			)
			.await
			.unwrap()
	};
	let stored = assert_json(owner.get("/api/registry/versions").await.unwrap(), 200);
	// Act
	let read_only = hint(owner.clone(), mcp.clone(), "lookup").await;
	let writes = hint(owner.clone(), mcp.clone(), "write").await;
	let unannotated = hint(owner.clone(), mcp.clone(), "plain").await;
	let unlisted = hint(owner.clone(), mcp.clone(), "missing").await;
	let not_http = hint(owner.clone(), "ftp://example.com/mcp".into(), "lookup").await;
	let unreachable = hint(owner.clone(), unreachable_endpoint.into(), "lookup").await;
	let reading_only = hint(reader, mcp.clone(), "lookup").await;
	let administrator = hint(
		fixture.deployment_administrator.clone(),
		mcp.clone(),
		"lookup",
	)
	.await;
	let anonymous = hint(fixture.anonymous.clone(), mcp, "lookup").await;
	// Assert
	assert_eq!(assert_json(read_only, 200), json!({"read_only_hint": true}));
	assert_eq!(assert_json(writes, 200), json!({"read_only_hint": false}));
	assert_eq!(
		assert_json(unannotated, 200),
		json!({"read_only_hint": null})
	);
	assert_eq!(
		assert_json(unlisted, 400)["error"],
		"the MCP server lists no tool with that name"
	);
	assert_json(not_http, 400);
	assert_eq!(
		assert_json(unreachable, 400)["error"],
		"the MCP server could not be read",
		"a refused connection must not describe the network behind the URL"
	);
	assert_eq!(reading_only.status_code(), 403, "{}", reading_only.text());
	assert_eq!(administrator.status_code(), 403, "{}", administrator.text());
	assert_eq!(anonymous.status_code(), 401, "{}", anonymous.text());
	assert_eq!(
		assert_json(owner.get("/api/registry/versions").await.unwrap(), 200),
		stored,
		"reading a hint registers and approves nothing"
	);
}

/// The endpoint is the owner's choice and nothing approves it first, so the
/// server reaches only public addresses and the Deployment's permitted origins
/// (ADR 0036), and a refusal reads like an unreachable server.
#[rstest]
#[tokio::test]
async fn an_mcp_tool_hint_reaches_no_private_address_the_deployment_does_not_permit(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange: a loopback MCP server that no permitted origin names.
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
	let requests = Arc::<std::sync::atomic::AtomicUsize>::default();
	let server = mcp_server(requests.clone()).await;
	let port = server.url.rsplit(':').next().unwrap().to_owned();
	let aimed = [
		format!("{}/mcp", server.url),
		format!("http://localhost:{port}/mcp"),
		"http://10.0.0.1/mcp".to_owned(),
		"http://169.254.169.254/mcp".to_owned(),
		"http://[::1]/mcp".to_owned(),
	];
	// Act
	let mut refused = Vec::new();
	for endpoint in &aimed {
		let response = owner
			.post(
				"/api/registry/mcp-tool-hint",
				&json!({"endpoint": endpoint, "tool_name": "lookup"}),
				"json",
			)
			.await
			.unwrap();
		refused.push((endpoint, response));
	}
	// Assert
	for (endpoint, response) in refused {
		assert_eq!(
			assert_json(response, 400)["error"],
			"the MCP server could not be read",
			"{endpoint}: a refused address must read like an unreachable server"
		);
	}
	assert_eq!(
		requests.load(std::sync::atomic::Ordering::SeqCst),
		0,
		"the MCP server received a request from an address the Deployment does not permit"
	);
}

#[rstest]
#[tokio::test]
async fn a_changed_read_only_declaration_needs_a_new_version(#[future] endpoint: EndpointFixture) {
	// Arrange
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
	let tool = |version: &str, replay: &str| {
		json!({"id":"lookup","version":version,"kind":"tool","name":{"en":"Lookup"},"description":{"en":"Acceptance"},
			"schema":{"type":"object"},
			"config":{"registry_node":NODE,"provider":"integration.http@1","operation":"invoke","default_alias":"lookup",
				"tier":"integration","narrow":{},
				"transport":{"transport":"http","endpoint":"https://example.com/lookup","replay":replay}}})
	};
	// Act
	let (first, first_body) = register(&owner, &tool("1.0.0", "unsafe")).await;
	let (declared, declared_body) = register(&owner, &tool("1.0.0", "read_only")).await;
	let (next, next_body) = register(&owner, &tool("1.1.0", "read_only")).await;
	let (repeated, _) = register(&owner, &tool("1.0.0", "unsafe")).await;
	// Assert
	assert_eq!(first, 200, "{first_body}");
	assert_eq!(declared, 409, "{declared_body}");
	assert!(
		declared_body.to_string().contains("immutable"),
		"{declared_body}"
	);
	assert_eq!(next, 200, "{next_body}");
	assert_eq!(repeated, 200, "the unchanged version still registers again");
}

const PLUGIN_SOURCE: &str =
	"https://github.com/acme/tools/tree/0123456789abcdef0123456789abcdef01234567";

fn plugin_files() -> Vec<PluginFile> {
	let file = |path: &str, text: &str| PluginFile {
		path: path.to_owned(),
		bytes: text.as_bytes().to_vec(),
	};
	vec![
		file(
			"plugin.json",
			&json!({"$schema":PLUGIN_SCHEMA,"name":"tools","version":"1.0.0"}).to_string(),
		),
		file(
			"skills/review/SKILL.md",
			"---\nname: review\ndescription: Reviews a diff.\n---\n\nRead the diff first.\n",
		),
		file("skills/review/references/guide.md", "Guide."),
		file("assets/logo.bin", "\u{0}binary"),
	]
}

/// A source that serves one fixed answer and counts how often it is asked.
struct FixedSource {
	answer: PluginFetch,
	calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl AgentPluginSource for FixedSource {
	async fn fetch(&self, _: PluginImportRequest) -> aidash_application::Result<PluginFetch> {
		self.calls.fetch_add(1, Ordering::SeqCst);
		Ok(self.answer.clone())
	}
}

fn fetched(files: Vec<PluginFile>, source: &str, denied: &[&str]) -> PluginFetch {
	PluginFetch::Fetched(FetchedPlugin {
		root: "plugins/tools".into(),
		source: source.to_owned(),
		files,
		denied: denied.iter().map(|path| (*path).to_owned()).collect(),
	})
}

fn import_request() -> PluginImportRequest {
	PluginImportRequest {
		url: "https://github.com/acme/tools".into(),
		plugin_path: None,
	}
}

async fn actor(
	fixture: &EndpointFixture,
	tenant: &str,
	subject: &str,
) -> aidash_server::authorization::identity::Actor {
	let issued = assert_json(
		fixture
			.deployment_administrator
			.post(
				&format!("/api/authorization/{tenant}/credentials"),
				&json!({"subject":subject}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	aidash_server::authorization::Authorization {
		pool: fixture.runtime.store.pool.clone(),
	}
	.authenticate(issued["token"].as_str().unwrap())
	.await
	.unwrap()
}

async fn import_plugin(
	fixture: &EndpointFixture,
	tenant: &str,
	subject: &str,
	answer: PluginFetch,
) -> (
	aidash_server::Result<
		aidash_server::apps::registry::serializers::agent_plugin_import::AgentPluginImportResult,
	>,
	usize,
) {
	let calls = Arc::new(AtomicUsize::new(0));
	let source = AgentPluginImport::new(Arc::new(FixedSource {
		answer,
		calls: calls.clone(),
	}));
	let result = aidash_server::apps::registry::services::agent_plugin_import::import(
		&fixture.runtime,
		&source,
		actor(fixture, tenant, subject).await,
		import_request(),
	)
	.await;
	(result, calls.load(Ordering::SeqCst))
}

fn plugin_agent(id: &str, instructions: &str, plugins: &[AgentPluginRef]) -> Value {
	let mut entry = agent(id, "1.0.0", "model");
	entry["config"]["instructions"] = json!(instructions);
	entry["config"]["agent_plugins"] = json!(plugins);
	entry
}

async fn copies(fixture: &EndpointFixture, tenant: &str) -> i64 {
	count(
		fixture,
		"SELECT count(*) FROM agent_plugin_copies WHERE tenant = $1",
		tenant,
	)
	.await
}

#[rstest]
#[tokio::test]
async fn an_import_keeps_one_immutable_copy_per_digest_and_only_for_a_caller_who_may_register(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	tenant(&fixture, "globex").await;
	let files = plugin_files();
	let loaded = agent_plugin::load(&files).unwrap();
	let later_commit =
		"https://github.com/acme/tools/tree/89abcdef0123456789abcdef0123456789abcdef";
	// Act
	let (denied, denied_calls) = import_plugin(
		&fixture,
		"acme",
		"reader",
		fetched(files.clone(), PLUGIN_SOURCE, &[]),
	)
	.await;
	let (first, _) = import_plugin(
		&fixture,
		"acme",
		"owner",
		fetched(files.clone(), PLUGIN_SOURCE, &["hooks/run.sh"]),
	)
	.await;
	let (again, _) = import_plugin(
		&fixture,
		"acme",
		"owner",
		fetched(files.clone(), later_commit, &[]),
	)
	.await;
	let (chosen, _) = import_plugin(
		&fixture,
		"acme",
		"owner",
		PluginFetch::Choose(vec!["plugins/a".into(), "plugins/b".into()]),
	)
	.await;
	let (elsewhere, _) = import_plugin(
		&fixture,
		"globex",
		"owner",
		fetched(files.clone(), later_commit, &[]),
	)
	.await;
	// Assert: a subject without registry.create causes no fetch and stores nothing.
	assert!(matches!(denied, Err(aidash_server::Error::Forbidden)));
	assert_eq!(denied_calls, 0);
	// The import returns the reference the Agent definition pins.
	let first = first.unwrap();
	assert_eq!(first.plugins, vec!["plugins/tools"]);
	let selected = first.selected.unwrap();
	assert_eq!(
		selected.reference,
		AgentPluginRef::of(&loaded, PLUGIN_SOURCE)
	);
	assert_eq!(selected.denied, vec!["hooks/run.sh"]);
	// A second import of the same files keeps the stored copy and its source.
	let again = again.unwrap().selected.unwrap();
	assert_eq!(again.reference, selected.reference);
	assert_eq!(copies(&fixture, "acme").await, 1);
	// A repository with several plugins stores nothing.
	let chosen = chosen.unwrap();
	assert_eq!(chosen.plugins, vec!["plugins/a", "plugins/b"]);
	assert!(chosen.selected.is_none());
	assert_eq!(copies(&fixture, "acme").await, 1);
	// Another Tenant keeps its own copy, under its own source.
	let elsewhere = elsewhere.unwrap().selected.unwrap();
	assert_eq!(elsewhere.reference.digest, selected.reference.digest);
	assert_eq!(elsewhere.reference.source, later_commit);
	assert_eq!(copies(&fixture, "globex").await, 1);
	// The row keeps the plugin, every fetched file (text or base64, in path order) and the
	// denied paths of the import that stored it.
	let stored: (Value, Value, Value) = sqlx::query_as(
		"SELECT plugin, files, denied FROM agent_plugin_copies WHERE tenant = 'acme'",
	)
	.fetch_one(fixture.runtime.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(stored.0, serde_json::to_value(&loaded).unwrap());
	assert_eq!(
		stored.1,
		json!([
			{"path":"assets/logo.bin","encoding":"base64","content":"AGJpbmFyeQ=="},
			{"path":"plugin.json","encoding":"utf8","content":String::from_utf8(files[0].bytes.clone()).unwrap()},
			{"path":"skills/review/SKILL.md","encoding":"utf8","content":String::from_utf8(files[1].bytes.clone()).unwrap()},
			{"path":"skills/review/references/guide.md","encoding":"utf8","content":"Guide."}
		])
	);
	assert_eq!(stored.2, json!(["hooks/run.sh"]));
}

#[rstest]
#[tokio::test]
async fn registration_reads_the_stored_copy_and_another_tenant_cannot(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	tenant(&fixture, "globex").await;
	let acme = credential(&fixture, "acme", "owner").await;
	let globex = credential(&fixture, "globex", "owner").await;
	for client in [&acme, &globex] {
		assert_eq!(register(client, &model("model")).await.0, 200);
	}
	let (imported, _) = import_plugin(
		&fixture,
		"acme",
		"owner",
		fetched(plugin_files(), PLUGIN_SOURCE, &[]),
	)
	.await;
	let reference = imported.unwrap().selected.unwrap().reference;
	let mut changed = reference.clone();
	changed.skills.push("deploy".into());
	let mut missing = reference.clone();
	missing.digest = format!("sha256:{}", "0".repeat(64));
	// Act
	let (registered, body) = register(
		&acme,
		&plugin_agent("tooled", "Help", std::slice::from_ref(&reference)),
	)
	.await;
	let (without_instructions, skills_only) = register(
		&acme,
		&plugin_agent("skills-only", "", std::slice::from_ref(&reference)),
	)
	.await;
	let (changed_status, changed_body) =
		register(&acme, &plugin_agent("changed", "Help", &[changed])).await;
	let (missing_status, missing_body) =
		register(&acme, &plugin_agent("missing", "Help", &[missing])).await;
	let (foreign_status, foreign_body) = register(
		&globex,
		&plugin_agent("foreign", "Help", std::slice::from_ref(&reference)),
	)
	.await;
	let digest = reference.digest.clone();
	let read = |tenant: &'static str| {
		let pool = fixture.runtime.store.pool.clone();
		let digest = digest.clone();
		async move {
			let mut tx = pool
				.begin(&TenancyScope::Tenant(TenantName::new(tenant).unwrap()))
				.await
				.unwrap();
			let stored =
				aidash_server::apps::registry::repositories::agent_plugins::get(&mut *tx, &digest)
					.await
					.unwrap();
			tx.rollback().await.unwrap();
			stored
		}
	};
	let (owned, foreign) = (read("acme").await, read("globex").await);
	// The Registry's own scope (the ORM path) decides the same way.
	let entry: aidash_server::registry::Entry = serde_json::from_value(plugin_agent(
		"orm",
		"Help",
		std::slice::from_ref(&reference),
	))
	.unwrap();
	let orm_owned = fixture
		.runtime
		.registry
		.validate_references(&TenantName::new("acme").unwrap(), &entry)
		.await;
	let orm_foreign = fixture
		.runtime
		.registry
		.validate_references(&TenantName::new("globex").unwrap(), &entry)
		.await;
	// Assert
	assert_eq!(registered, 200, "{body}");
	assert_eq!(
		body["config"]["agent_plugins"][0]["digest"],
		json!(reference.digest)
	);
	assert_eq!(without_instructions, 200, "{skills_only}");
	assert_eq!(changed_status, 409, "{changed_body}");
	assert!(
		changed_body.to_string().contains("AGENT_PLUGIN_CHANGED"),
		"{changed_body}"
	);
	assert_eq!(missing_status, 400, "{missing_body}");
	assert!(
		missing_body.to_string().contains("AGENT_PLUGIN_MISSING"),
		"{missing_body}"
	);
	assert_eq!(foreign_status, 400, "{foreign_body}");
	assert!(
		foreign_body.to_string().contains("AGENT_PLUGIN_MISSING"),
		"{foreign_body}"
	);
	assert_eq!(owned.unwrap().plugin.digest, reference.digest);
	assert!(foreign.is_none());
	orm_owned.unwrap();
	assert!(orm_foreign.is_err());
}
