//! Tenant Secret readiness facts over the production router and PostgreSQL.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../execution/tests/support/endpoint.rs"]
mod endpoint_fixtures;
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
use aidash_application::tenant_secrets::Put;
use aidash_application::tenant_secrets::Service as SecretService;
use aidash_application::tenant_secrets::readiness::{Fact, Kind, Repair, Status};
use aidash_domain::registry::Entry;
use aidash_server::apps::identity::repositories::tenant_secrets::NativeRepository;
use aidash_server::authorization::{Authorization, policy::PolicyBundle};
use endpoint_fixtures::{EndpointFixture, endpoint, tenant_root};
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const TEST_BOUND: Duration = Duration::from_secs(600);

fn entry(value: Value) -> Entry {
	serde_json::from_value(value).expect("a Registry entry fixture")
}

fn model(_node: &str) -> Entry {
	entry(json!({
		"id":"model","version":"1.0.0","kind":"model",
		"name":{"en":"Model"},"description":{"en":"Model"},
		"config":{"provider":"openrouter","model_id":"fixture","endpoint":"https://fixture.invalid","context_window":200000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}
	}))
}

fn tool(node: &str, id: &str, host: &str, secret: Option<&str>) -> Entry {
	let mut transport = json!({
		"transport":"http",
		"endpoint":format!("https://{host}/v1/"),
		"replay":"read_only"
	});
	if let Some(secret) = secret {
		transport["secret"] = json!(secret);
	}
	entry(json!({
		"id":id,"version":"1.0.0","kind":"tool",
		"name":{"en":id},"description":{"en":"Bound tool"},
		"config":{
			"registry_deployment":node,
			"provider":"integration.http@1",
			"operation":"invoke",
			"default_alias":id,
			"tier":"integration",
			"transport":transport
		}
	}))
}

/// A tool that accepts only the Run's person's own secret (ADR 0100).
fn personal_tool(node: &str, id: &str, host: &str, secret: &str) -> Entry {
	let mut entry = tool(node, id, host, Some(secret));
	entry.config["transport"]["secret_owner"] = json!("personal");
	entry
}

fn external_service(secret: &str) -> Entry {
	entry(json!({
		"id":"images","version":"1.0.0","kind":"external_service",
		"name":{"en":"Images"},"description":{"en":"Bound External service"},
		"config":{
			"base_url":"https://images.example.test/v1/",
			"methods":["GET"],
			"secret":secret,
			"authentication":{"type":"bearer"}
		}
	}))
}

fn agent(_node: &str, id: &str, bindings: Value) -> Entry {
	entry(json!({
		"id":id,"version":"1.0.0","kind":"agent",
		"name":{"en":id},"description":{"en":"Readiness fixture"},
		"config":{
			"schema_version":1,
			"instructions":"Use the bound tools for readiness.",
			"model":{"id":"model","version":"1.0.0"},
			"bindings":bindings,
			"remove_default":[]
		}
	}))
}

fn binding(node: &str, kind: &str, id: &str) -> Value {
	json!({
		"kind":kind,
		"target":{"registry_deployment":node,"id":id,"version":"1.0.0"},
		"narrow":{}
	})
}

fn bundle(manage: bool) -> Value {
	let mut actions = vec!["registry.read", "secret.use"];
	if manage {
		actions.push("secret.manage");
	}
	json!({
		"tenant":"alpha",
		"subjects":{"viewer":{"kind":"user","roles":["manager"]}},
		"roles":{"manager":{"inherits":["reader"]},"reader":{}},
		"policies":[{
			"id":"all","effect":"allow","subjects":{"roles":["reader"]},
			"actions":actions,"resources":{"kinds":["*"]}
		}]
	})
}

async fn postgres_store(
	f: &EndpointFixture,
	current: &str,
) -> Arc<aidash_server::apps::identity::repositories::secret_store::PostgresStore> {
	Arc::new(
		aidash_server::apps::identity::repositories::secret_store::PostgresStore::new(
			f.runtime.store.control_pool.clone(),
			current.into(),
			vec![],
		)
		.await
		.unwrap(),
	)
}

/// The fixture's production service, with a plaintext PostgreSQL Store so the
/// test can create the Tenant Secrets it needs without a broker.
async fn secrets(f: &EndpointFixture) -> Arc<SecretService> {
	let store = postgres_store(f, &"33".repeat(32)).await;
	Arc::new(SecretService {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
		}),
		store: store.clone(),
		reader: Some(store),
		calls: None,
		fingerprint_key: "tenant-secret-readiness-at-least-32-bytes".into(),
		max_per_tenant: 64,
	})
}

async fn put(service: &SecretService, tenant: &str, name: &str, host: &str) -> i64 {
	service
		.put(
			tenant,
			name,
			Put {
				expected_revision: 0,
				value: Some(format!("readiness-canary-{tenant}-{name}").into()),
				hosts: Some(vec![host.into()]),
			},
			"actor",
		)
		.await
		.expect("the Tenant Secret is stored")
		.revision
}

async fn set_policy(f: &EndpointFixture, bundle: Value) {
	Authorization {
		pool: f.runtime.store.pool.clone(),
	}
	.replace(
		"alpha",
		0,
		serde_json::from_value::<PolicyBundle>(bundle).expect("a policy bundle"),
		"deployment_administrator",
	)
	.await
	.expect("the Tenant policy is created");
}

async fn token(f: &EndpointFixture) -> String {
	Authorization {
		pool: f.runtime.store.pool.clone(),
	}
	.issue_credential("alpha", "viewer", 3600, "deployment_administrator")
	.await
	.expect("the Subject Credential is issued")
	.token
}

async fn read_facts(f: &EndpointFixture, token: &str, agent: &str) -> Vec<Fact> {
	let response = reqwest::Client::new()
		.get(format!(
			"{}/api/registry/{agent}/1.0.0/secret-readiness",
			f.server.url
		))
		.bearer_auth(token)
		.send()
		.await
		.unwrap();
	let status = response.status().as_u16();
	let body = response.text().await.unwrap();
	assert_eq!(status, 200, "{body}");
	serde_json::from_str(&body).expect("readiness facts")
}

fn fact<'a>(facts: &'a [Fact], item: &str) -> &'a Fact {
	facts
		.iter()
		.find(|fact| fact.item == item)
		.unwrap_or_else(|| panic!("missing readiness fact for {item}: {facts:?}"))
}

#[rstest]
#[timeout(TEST_BOUND)]
#[tokio::test]
async fn agent_readiness_reports_every_status_and_repair(#[future] endpoint: EndpointFixture) {
	let mut f = endpoint.await;
	tenant_root(&f.runtime, "alpha").await;
	let service = secrets(&f).await;
	put(&service, "alpha", "crm-key", "api.example.test").await;
	let image = put(&service, "alpha", "image-key", "images.example.test").await;
	let missing = put(&service, "alpha", "missing-key", "sync.example.test").await;
	f.runtime.store.tenant_secrets = Some(service.clone());
	f.runtime.registry = f.runtime.registry.clone().with_tenant_secrets(true);
	f.context.set_singleton(f.runtime.clone());
	let node = f.runtime.store.deployment.to_string();
	for entry in [
		model(&node),
		tool(&node, "crm", "api.example.test", Some("crm-key")),
		external_service("image-key"),
		tool(&node, "sync", "sync.example.test", Some("missing-key")),
		agent(
			&node,
			"research",
			json!([
				binding(&node, "tool", "crm"),
				binding(&node, "external_service", "images"),
				binding(&node, "tool", "sync")
			]),
		),
	] {
		f.runtime
			.registry
			.register(
				&aidash_domain::tenancy::TenantName::new("alpha").unwrap(),
				entry,
			)
			.await
			.unwrap();
	}
	// Replacing hosts and deleting a Secret after registration is what makes
	// the two repairable statuses observable.
	service
		.put(
			"alpha",
			"image-key",
			Put {
				expected_revision: image,
				value: None,
				hosts: Some(vec!["other.example.test".into()]),
			},
			"actor",
		)
		.await
		.unwrap();
	service
		.delete("alpha", "missing-key", missing, "actor")
		.await
		.unwrap();
	set_policy(&f, bundle(true)).await;
	let managing = token(&f).await;
	let facts = read_facts(&f, &managing, "research").await;
	assert_eq!(facts.len(), 3);
	assert_eq!(fact(&facts, "crm@1.0.0").status, Status::Ready);
	assert_eq!(fact(&facts, "crm@1.0.0").kind, Kind::Tool);
	assert_eq!(fact(&facts, "crm@1.0.0").repair, Repair::None);
	assert_eq!(
		fact(&facts, "images@1.0.0").status,
		Status::SecretHostsMismatch
	);
	assert_eq!(fact(&facts, "images@1.0.0").kind, Kind::ExternalService);
	assert_eq!(fact(&facts, "images@1.0.0").repair, Repair::ManageSecret);
	assert_eq!(fact(&facts, "sync@1.0.0").status, Status::SecretMissing);
	assert_eq!(fact(&facts, "sync@1.0.0").repair, Repair::ManageSecret);
	assert_eq!(
		fact(&facts, "sync@1.0.0").tenant_secret_name.as_deref(),
		Some("missing-key")
	);

	endpoint_fixtures::replace_policy(&f.runtime.store, "alpha", bundle(false)).await;
	let asking = token(&f).await;
	let facts = read_facts(&f, &asking, "research").await;
	assert_eq!(
		fact(&facts, "images@1.0.0").repair,
		Repair::AskAdministrator
	);
	assert_eq!(fact(&facts, "sync@1.0.0").repair, Repair::AskAdministrator);

	f.runtime.store.tenant_secrets = None;
	f.context.set_singleton(f.runtime.clone());
	let facts = read_facts(&f, &asking, "research").await;
	for item in ["crm@1.0.0", "images@1.0.0", "sync@1.0.0"] {
		assert_eq!(
			fact(&facts, item).status,
			Status::SecretsUnavailableOnDeployment
		);
		assert_eq!(fact(&facts, item).repair, Repair::DeploymentAdministrator);
	}

	// A Store that seals hosts but has no broker cannot attach a value.
	f.runtime.store.tenant_secrets = Some(Arc::new(SecretService {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
		}),
		store: service.store.clone(),
		reader: None,
		calls: None,
		fingerprint_key: "tenant-secret-readiness-at-least-32-bytes".into(),
		max_per_tenant: 64,
	}));
	f.context.set_singleton(f.runtime.clone());
	let facts = read_facts(&f, &asking, "research").await;
	for item in ["crm@1.0.0", "images@1.0.0", "sync@1.0.0"] {
		assert_eq!(fact(&facts, item).status, Status::BrokerNotConfigured);
		assert_eq!(fact(&facts, item).repair, Repair::DeploymentAdministrator);
	}
}

#[rstest]
#[timeout(TEST_BOUND)]
#[tokio::test]
async fn a_personal_only_tool_registers_without_a_shared_row_and_has_no_readiness_fact(
	#[future] endpoint: EndpointFixture,
) {
	let mut f = endpoint.await;
	tenant_root(&f.runtime, "alpha").await;
	let service = secrets(&f).await;
	let shared = put(&service, "alpha", "shared-key", "api.example.test").await;
	f.runtime.store.tenant_secrets = Some(service.clone());
	f.runtime.registry = f.runtime.registry.clone().with_tenant_secrets(true);
	f.context.set_singleton(f.runtime.clone());
	let node = f.runtime.store.deployment.to_string();
	let tenant = aidash_domain::tenancy::TenantName::new("alpha").unwrap();
	// A shared definition still needs a row that exists and records its host.
	assert!(
		f.runtime
			.registry
			.register(
				&tenant,
				tool(&node, "orphan", "api.example.test", Some("no-row"))
			)
			.await
			.is_err()
	);
	// A personal-only definition needs neither: no `personal-key` row exists, and
	// `shared-key` records a different host than `elsewhere`.
	for entry in [
		model(&node),
		personal_tool(&node, "mine", "api.example.test", "personal-key"),
		personal_tool(&node, "elsewhere", "other.example.test", "shared-key"),
		tool(&node, "shared", "api.example.test", Some("shared-key")),
	] {
		f.runtime.registry.register(&tenant, entry).await.unwrap();
	}
	// A personal-only definition still validates the secret name.
	assert!(
		f.runtime
			.registry
			.register(
				&tenant,
				personal_tool(&node, "badname", "api.example.test", "Not A Name"),
			)
			.await
			.is_err()
	);
	service
		.put(
			"alpha",
			"shared-key",
			Put {
				expected_revision: shared,
				value: None,
				hosts: Some(vec!["moved.example.test".into()]),
			},
			"actor",
		)
		.await
		.unwrap();
	f.runtime
		.registry
		.register(
			&tenant,
			agent(
				&node,
				"research",
				json!([
					binding(&node, "tool", "mine"),
					binding(&node, "tool", "elsewhere"),
					binding(&node, "tool", "shared")
				]),
			),
		)
		.await
		.unwrap();
	set_policy(&f, bundle(true)).await;
	let managing = token(&f).await;
	let facts = read_facts(&f, &managing, "research").await;
	assert_eq!(facts.len(), 1, "{facts:?}");
	assert_eq!(
		fact(&facts, "shared@1.0.0").status,
		Status::SecretHostsMismatch
	);
}

#[rstest]
#[timeout(TEST_BOUND)]
#[tokio::test]
async fn agent_readiness_never_reads_another_tenants_secret(#[future] endpoint: EndpointFixture) {
	let mut f = endpoint.await;
	for tenant in ["alpha", "beta"] {
		tenant_root(&f.runtime, tenant).await;
	}
	let service = secrets(&f).await;
	put(&service, "beta", "tenant-only-key", "api.example.test").await;
	let alpha = put(&service, "alpha", "tenant-only-key", "api.example.test").await;
	f.runtime.store.tenant_secrets = Some(service.clone());
	f.runtime.registry = f.runtime.registry.clone().with_tenant_secrets(true);
	f.context.set_singleton(f.runtime.clone());
	let node = f.runtime.store.deployment.to_string();
	for entry in [
		model(&node),
		tool(&node, "crm", "api.example.test", Some("tenant-only-key")),
		agent(&node, "research", json!([binding(&node, "tool", "crm")])),
	] {
		f.runtime
			.registry
			.register(
				&aidash_domain::tenancy::TenantName::new("alpha").unwrap(),
				entry,
			)
			.await
			.unwrap();
	}
	// Only the other Tenant keeps this name and its recorded host.
	service
		.delete("alpha", "tenant-only-key", alpha, "actor")
		.await
		.unwrap();
	set_policy(&f, bundle(true)).await;
	let token = token(&f).await;
	let facts = read_facts(&f, &token, "research").await;
	assert_eq!(facts.len(), 1);
	assert_eq!(
		fact(&facts, "crm@1.0.0").status,
		Status::SecretMissing,
		"beta's Tenant Secret must not make alpha ready"
	);
	assert_eq!(fact(&facts, "crm@1.0.0").repair, Repair::ManageSecret);

	// A Deployment Administrator is not a subject of any Tenant.
	let response = reqwest::Client::new()
		.get(format!(
			"{}/api/registry/research/1.0.0/secret-readiness",
			f.server.url
		))
		.bearer_auth(&f.runtime.config.api_token)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status().as_u16(), 403);
}
