//! #499 recipient-Tenant acceptance: an author publishes a Secret slot, the
//! recipient fills it with its own Tenant Secret, and every subsequent grant
//! names the recipient. The author's secret is never read or copied.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.

#[path = "../../execution/tests/support/endpoint.rs"]
mod endpoint_fixtures;
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;

use aidash_application::tenant_secrets::{
	CallSecret, Put, SecretCall, Service as SecretService, endpoint_destination,
};
use aidash_application::tenant_secrets::{SecretCallGrant, SecretCalls, SecretRoute};
use aidash_capability::tenant_secret::{Attach, SecretSubject};
use aidash_domain::{registry::Entry, tenancy::TenantName};
use aidash_server::apps::identity::repositories::secret_store::PostgresStore;
use aidash_server::apps::identity::repositories::tenant_secrets::NativeRepository;
use aidash_server::authorization::{Authorization, policy::PolicyBundle};
use endpoint_fixtures::{EndpointFixture, endpoint, tenant_root};
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

const TEST_BOUND: Duration = Duration::from_secs(600);
const ENDPOINT: &str = "https://production.example/tool";

fn reference(id: &str) -> aidash_server::registry::EntityRef {
	aidash_server::registry::EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}

fn tool(node: &str, id: &str, secret: &str) -> Entry {
	serde_json::from_value(json!({
		"id": id,
		"version": "1.0.0",
		"kind": "tool",
		"name": {"en": id},
		"description": {"en": "Recipient Tenant fixture"},
		"config": {
			"registry_deployment": node,
			"provider": "integration.http@1",
			"operation": "invoke",
			"default_alias": "plugin_0",
			"tier": "integration",
			"narrow": {},
			"transport": {
				"transport": "http",
				"endpoint": ENDPOINT,
				"replay": "read_only",
				"secret": secret
			}
		}
	}))
	.expect("a Tool entry")
}

fn bundle(tenant: &str) -> Value {
	json!({
		"tenant": tenant,
		"subjects": {"viewer": {"kind": "user", "roles": ["manager"]}},
		"roles": {"manager": {"inherits": ["reader"]}, "reader": {}},
		"policies": [{
			"id": "all",
			"effect": "allow",
			"subjects": {"roles": ["reader"]},
			"actions": ["*"],
			"resources": {"kinds": ["*"]}
		}]
	})
}

async fn set_policy(f: &EndpointFixture, tenant: &str) {
	Authorization {
		pool: f.runtime.store.pool.clone(),
	}
	.replace(
		tenant,
		0,
		serde_json::from_value::<PolicyBundle>(bundle(tenant)).expect("a policy bundle"),
		"deployment_administrator",
	)
	.await
	.expect("the Tenant policy is created");
}

async fn token(f: &EndpointFixture, tenant: &str) -> String {
	Authorization {
		pool: f.runtime.store.pool.clone(),
	}
	.issue_credential(tenant, "viewer", 3600, "deployment_administrator")
	.await
	.unwrap()
	.token
}

async fn approve(f: &EndpointFixture, tenant: &str, entry: &aidash_server::registry::EntityRef) {
	Authorization {
		pool: f.runtime.store.pool.clone(),
	}
	.set_catalog(
		tenant,
		&aidash_domain::nodes::NodeName::derived(tenant),
		entry,
		0,
		true,
		"deployment_administrator",
	)
	.await
	.unwrap();
}

async fn request(
	f: &EndpointFixture,
	token: &str,
	method: &str,
	path: &str,
	body: Value,
) -> (u16, Value) {
	let response = reqwest::Client::new()
		.request(method.parse().unwrap(), format!("{}{path}", f.server.url))
		.bearer_auth(token)
		.json(&body)
		.send()
		.await
		.unwrap();
	let status = response.status().as_u16();
	let body = response.text().await.unwrap();
	(
		status,
		serde_json::from_str(&body).unwrap_or(Value::String(body)),
	)
}

#[derive(Clone, Default)]
struct Recorded(Arc<Mutex<Vec<SecretCallGrant>>>);
struct FakeCalls(Recorded);
#[async_trait::async_trait]
impl SecretCalls for FakeCalls {
	async fn route(&self, grant: SecretCallGrant) -> aidash_application::Result<SecretRoute> {
		self.0.0.lock().unwrap().push(grant);
		Ok(SecretRoute {
			base: reqwest::Url::parse("https://broker.example/v1/forward").unwrap(),
			token: "canary-capability-token".into(),
		})
	}
}

#[rstest]
#[timeout(TEST_BOUND)]
#[tokio::test]
async fn a_recipient_fills_the_author_slot_with_its_own_secret_and_every_grant_names_it(
	#[future] endpoint: EndpointFixture,
) {
	let mut f = endpoint.await;
	for tenant in ["author", "recipient"] {
		tenant_root(&f.runtime, tenant).await;
		set_policy(&f, tenant).await;
	}
	let author_token = token(&f, "author").await;
	let recipient_token = token(&f, "recipient").await;

	// The installer keeps a Tenant Secret of its own; the author also has a
	// secret under the package's slot name, which must never be read or copied.
	let grants = Recorded::default();
	let store = Arc::new(
		PostgresStore::new(
			f.runtime.store.control_pool.clone(),
			"33".repeat(32).into(),
			vec![],
		)
		.await
		.expect("the fixture Secret Store"),
	);
	let service = Arc::new(SecretService {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
		}),
		store: store.clone(),
		reader: None,
		calls: Some(Arc::new(FakeCalls(grants.clone()))),
		fingerprint_key: "tenant-secret-recipient-test-at-least-32-bytes".into(),
		max_per_tenant: 64,
	});
	f.runtime.store.tenant_secrets = Some(service.clone());
	f.runtime.registry = f.runtime.registry.clone().with_tenant_secrets(true);
	f.context.set_singleton(f.runtime.clone());

	let recipient = service
		.put(
			"recipient",
			"recipient-key",
			Put {
				expected_revision: 0,
				value: Some("recipient-own-secret-canary".into()),
				hosts: Some(vec!["production.example".into()]),
			},
			"actor",
		)
		.await
		.unwrap();
	let author = service
		.put(
			"author",
			"publisher-key",
			Put {
				expected_revision: 0,
				value: Some("author-secret-never-reused".into()),
				hosts: Some(vec!["production.example".into()]),
			},
			"actor",
		)
		.await
		.unwrap();
	assert_ne!(author.id, recipient.id);

	let node = f.runtime.store.deployment.to_string();
	f.runtime
		.registry
		.register(
			&TenantName::new("author").unwrap(),
			tool(&node, "publisher-tool", "publisher-key"),
		)
		.await
		.unwrap();
	approve(&f, "author", &reference("publisher-tool")).await;

	let published = request(
		&f,
		&author_token,
		"POST",
		"/api/marketplace/packages",
		json!({
			"source": reference("publisher-tool"),
			"package_id": "recipient-case",
			"author": "display only",
			"idempotency_key": Uuid::new_v4()
		}),
	)
	.await;
	assert_eq!(published.0, 200, "{published:?}");
	// A package is readable only by its audience; the author shares it with the
	// recipient Tenant that will install it.
	let shared = request(
		&f,
		&author_token,
		"PUT",
		&format!(
			"/api/marketplace/packages/{}/audience",
			published.1["key"].as_str().unwrap()
		),
		json!({
			"expected_revision": 1,
			"tenants": ["author", "recipient"]
		}),
	)
	.await;
	assert_eq!(shared.0, 200, "{shared:?}");

	let readiness = request(
		&f,
		&recipient_token,
		"GET",
		&format!(
			"/api/marketplace/packages/{}/secret-readiness",
			published.1["key"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(readiness.0, 200, "{readiness:?}");
	assert_eq!(
		readiness.1,
		json!([{
			"item": "publisher-key", "kind": "slot", "status": "ready", "repair": "none"
		}])
	);
	let disclosure = readiness.1.to_string();
	assert!(!disclosure.contains("recipient-own-secret-canary"));
	assert!(!disclosure.contains("author-secret-never-reused"));
	assert!(!disclosure.contains("recipient-key"));

	let installed = request(
		&f,
		&recipient_token,
		"POST",
		&format!(
			"/api/marketplace/packages/{}/install",
			published.1["key"].as_str().unwrap()
		),
		json!({
			"digest": published.1["digest"],
			"config": {},
			"bindings": [],
			"secrets": {"publisher-key": "recipient-key"},
			"idempotency_key": Uuid::new_v4()
		}),
	)
	.await;
	assert_eq!(installed.0, 200, "{installed:?}");
	// The installed copy names the recipient's Tenant Secret, not the author's.
	assert_eq!(
		installed.1["entry"]["config"]["transport"]["secret"],
		"recipient-key"
	);
	assert_eq!(installed.1["secrets"]["publisher-key"], "recipient-key");
	assert_eq!(installed.1["entry"]["installation"]["tenant"], "recipient");

	// A call for the installed Tool asks the broker for the recipient's Tenant
	// Secret on the recipient Tenant. The fake route succeeds without a value.
	let call = service
		.call_secret(
			"recipient",
			"recipient-key",
			ENDPOINT,
			SecretCall {
				destination: endpoint_destination(ENDPOINT).unwrap(),
				methods: vec!["POST".into()],
				attach: Attach::Bearer,
				subject: SecretSubject::Run {
					run: "run-recipient".into(),
					call: Uuid::new_v4(),
				},
				max_response_bytes: Some(256_000),
			},
		)
		.await
		.unwrap();
	assert!(matches!(call, CallSecret::Brokered { .. }));

	{
		let grants = grants.0.lock().unwrap();
		assert_eq!(grants.len(), 1);
		assert_eq!(grants[0].tenant, "recipient");
		assert_eq!(grants[0].secret, recipient.id);
		assert_eq!(grants[0].destination.origin, "https://production.example");
		assert_eq!(grants[0].version, "1");
		assert_ne!(grants[0].secret, author.id);
		assert!(grants.iter().all(|grant| grant.tenant != "author"));
		assert!(grants.iter().all(|grant| grant.secret != author.id));
	}

	// The author's secret still exists under the author Tenant and was not copied.
	let author_secrets = service.list("author").await.unwrap();
	assert!(
		author_secrets.iter().any(|secret| secret.id == author.id),
		"the author's Tenant Secret must remain untouched"
	);
	assert!(
		author_secrets
			.iter()
			.all(|secret| secret.id != recipient.id)
	);
}
