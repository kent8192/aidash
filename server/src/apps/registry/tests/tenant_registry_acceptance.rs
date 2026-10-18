#![allow(clippy::disallowed_methods)]
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "../../execution/tests/support/endpoint.rs"]
mod support;

use aidash_domain::identity::tenancy::{ADMINISTRATORS, MEMBERS, initial_policy};
use aidash_domain::policy::{Policy, ResourceSelector, Subject, SubjectKind, SubjectSelector};
use reinhardt::test::APIClient;
use reinhardt::test::fixtures::api_client_from_url;
use rstest::rstest;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
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
		.fetch_one(fixture.runtime.store.pool.driver())
		.await
		.unwrap()
}

#[rstest]
#[tokio::test]
async fn a_tenant_registers_approves_and_runs_its_own_agent_without_a_deployment_administrator(
	#[future] endpoint: EndpointFixture,
) {
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
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
	let unapproved = versions
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
	assert_ne!(read, 200);
	assert_ne!(reference, 200);
	assert_ne!(approval_status, 200);
	assert_eq!(cross, 403);
	assert!(
		listed
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
	let (first, created) = register(&acme, &unnamed).await;
	let (other_tenant, elsewhere) = register(&globex, &unnamed).await;
	let (replayed, again) = register(&acme, &unnamed).await;
	let mut changed = unnamed.clone();
	changed["config"]["instructions"] = json!("Changed");
	let (conflict, _) = register(&acme, &changed).await;
	let reader = credential(&fixture, "acme", "reader").await;
	reader.set_header("Idempotency-Key", &key).await.unwrap();
	let (denied, _) = register(&reader, &unnamed).await;
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
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let reader = credential(&fixture, "acme", "reader").await;
	let member = credential(&fixture, "acme", "member").await;
	reader
		.set_header("Idempotency-Key", &Uuid::new_v4().to_string())
		.await
		.unwrap();
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
	let fixture = endpoint.await;
	tenant(&fixture, "acme").await;
	let owner = credential(&fixture, "acme", "owner").await;
	let versions = assert_json(owner.get("/api/registry/versions").await.unwrap(), 200);
	let bundle = versions
		.as_array()
		.unwrap()
		.iter()
		.find(|version| version["entry"]["id"] == "aidash.bundle.outbound_get")
		.unwrap_or_else(|| panic!("the Deployment offers outbound requests: {versions}"))
		.clone();
	assert_eq!(bundle["needs_approval"], true);
	assert_eq!(bundle["approval"], Value::Null);
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
	assert_eq!(alone, 400, "{refusal}");
	assert_eq!(closure, 200);
}

#[rstest]
#[tokio::test]
async fn agent_groups_need_group_authority_and_approval_never_restores_a_disabled_agent(
	#[future] endpoint: EndpointFixture,
) {
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
	assert_eq!(named, 403);
	assert_eq!(defaulted, 200);
	assert_eq!(withdrawn, 200);
	assert_eq!(restored, 200);
	let after = &policy(&fixture, "acme").await["bundle"]["subjects"][&subject];
	assert_eq!(after["groups"], json!([MEMBERS]));
	assert_eq!(after["enabled"], false);
}
