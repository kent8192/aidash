//! Approved local execution through the application's native HTTP routes.
use crate::endpoint::{
	ClientFuture, EndpointFixture, EndpointFuture, anonymous_client, assert_json, endpoint,
};
use crate::provider_fixtures::{CompletionFixture, final_server};
use aidash_server::{authorization::Authorization, domain::qualified_agent, registry::EntityRef};
use reinhardt::test::APIClient;
use rstest::fixture;
use serde_json::{Value, json};
use std::future::Future;
use std::sync::Arc;
use uuid::Uuid;

#[allow(dead_code)] // Suites exercise different parts of this shared admitted execution.
pub struct ExecutionFixture {
	pub app: EndpointFixture,
	pub provider: CompletionFixture,
	pub subject: Arc<APIClient>,
	pub subject_token: String,
	pub policy: Value,
	pub task: Uuid,
}

#[fixture]
pub fn execution(
	#[default("aidash://endpoint-test")] node_id: &'static str,
	#[with(node_id)] endpoint: EndpointFuture,
	#[from(anonymous_client)]
	#[with(endpoint.clone())]
	subject_client: ClientFuture,
	#[future] final_server: CompletionFixture,
) -> impl Future<Output = ExecutionFixture> {
	let endpoint = Box::pin(endpoint);
	let final_server = Box::pin(final_server);
	async move {
		let app = endpoint.await;
		assert_eq!(app.runtime.config.deployment, node_id);
		let provider = final_server.await;
		let node = app.runtime.config.deployment.as_str();
		let agent = qualified_agent(
			&aidash_domain::nodes::NodeAddress::derived(node, "acme"),
			"research",
			"1.0.0",
		);
		// Peer flows still name the executing Node by its Deployment id (#241).
		let peer_agent = aidash_domain::agent_subject(node, "research", "1.0.0");
		let policy = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},agent:{"kind":"agent"},peer_agent:{"kind":"agent"}},
			"policies":[{"id":"approved-work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"call-unapproved","effect":"allow","subjects":{"any":true},"actions":["tool.call_unapproved"],"resources":{"kinds":["tool"]}}]});
		crate::endpoint::tenant_root(&app.runtime, "acme").await;
		assert_json(
			app.deployment_administrator
				.post(
					"/api/authorization/acme",
					&json!({"expected_revision":0,"bundle":policy}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
		let authorization = Authorization {
			pool: app.runtime.store.pool.clone(),
		};
		let endpoint = &provider.server.url;
		// The provider fixture is a Keyless endpoint on loopback (ADR 0036).
		app.runtime
			.store
			.keyless
			.permit_origin_of(endpoint)
			.unwrap();
		for (kind, id, config) in [
			(
				"model",
				"model",
				json!({"provider":"openrouter","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
			),
			(
				"tool",
				"http",
				json!({"registry_deployment":node,"provider":"integration.http@1","operation":"invoke","default_alias":"plugin_0","tier":"integration","narrow":{},"transport":{"transport":"http","endpoint":format!("{endpoint}/effect"),"replay":"idempotent"}}),
			),
			(
				"agent",
				"research",
				json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Test approved work","schema_version":1,"bindings":[{"kind":"tool","target":{"registry_deployment":node,"id":"http","version":"1.0.0"},"alias":"plugin_0","narrow":{}}],"remove_default":[]}),
			),
		] {
			let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":config});
			app.runtime
				.registry
				.register(&tenant, serde_json::from_value(entry).unwrap())
				.await
				.unwrap();
			authorization
				.set_catalog(
					"acme",
					&aidash_domain::nodes::NodeName::derived("acme"),
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
		}
		// The first Subject Credential of a Tenant comes from the host (#484).
		let subject = subject_client.await;
		let subject_token =
			crate::endpoint::subject_credential(&app.runtime.store, "acme", "alice").await;
		subject
			.set_header("Authorization", &format!("Bearer {subject_token}"))
			.await
			.unwrap();
		let workspace = assert_json(
			subject
				.post(
					"/api/workspaces",
					&json!({"title":"Approved task","goal":"Use approved tools"}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		let task = assert_json(
			subject
				.post(
					&format!(
						"/api/workspaces/{}/tasks",
						workspace["id"].as_str().unwrap()
					),
					&json!({"title":"Research","description":"Use approved tools"}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		ExecutionFixture {
			app,
			provider,
			subject,
			subject_token,
			policy,
			task: Uuid::parse_str(task["id"].as_str().unwrap()).unwrap(),
		}
	}
}
