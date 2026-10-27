//! Native persistence, policy selection, ownership and Store destruction for #554.
#![allow(clippy::disallowed_methods)] // Shared fixtures provision isolated databases outside a Tenant scope.
#[path = "../../execution/tests/support/endpoint.rs"]
mod endpoint_fixtures;
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "support/personal_credentials.rs"]
mod personal_credentials;
use aidash_application::{
	oauth_clients::{Put, Service},
	ports::oauth::ConnectionPolicy,
	tenant_secrets::Ownership,
};
use aidash_server::apps::identity::repositories::{
	connection_policy::NativeConnectionPolicy, oauth_clients::NativeRepository,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use endpoint_fixtures::{EndpointFixture, endpoint, tenant_root};
use rstest::rstest;
use secrecy::ExposeSecret;
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn input(owner: Ownership, secret: Option<&str>) -> Put {
	Put {
		owner,
		issuer: "https://AUTH.example.test/issuer/".into(),
		client_id: "client-id".into(),
		client_secret: secret.map(Into::into),
		expected_revision: 0,
	}
}

#[rstest]
#[timeout(Duration::from_secs(600))]
#[tokio::test]
async fn registered_clients_are_private_normalized_and_destroyed_on_retirement(
	#[future] endpoint: EndpointFixture,
) {
	let mut f = endpoint.await;
	tenant_root(&f.runtime, "alpha").await;
	tenant_root(&f.runtime, "beta").await;
	let pool = f.runtime.store.control_pool.clone();
	let store = personal_credentials::configure(&mut f.runtime).await;
	let service = Service {
		repository: Arc::new(NativeRepository { pool: pool.clone() }),
		store: Some(store.clone()),
	};
	let policy = NativeConnectionPolicy { pool };
	let unrelated = personal_credentials::seed(&f.runtime, "beta", "other").await;
	let shared = service
		.put(
			"alpha",
			"admin",
			None,
			input(Ownership::Tenant, Some("shared-canary")),
		)
		.await
		.unwrap();
	let alice = service
		.put(
			"alpha",
			"alice",
			None,
			input(Ownership::Me, Some("alice-canary")),
		)
		.await
		.unwrap();
	let bob = service
		.put("alpha", "bob", None, input(Ownership::Me, None))
		.await
		.unwrap();
	let public_row = service.get("alpha", "bob", bob.id).await.unwrap();
	assert!(public_row.secret_resource.is_none());
	assert!(public_row.pinned_version.is_none());
	let (personal, tenant) = policy
		.registered_clients("alpha", "alice", "https://auth.example.test/issuer/")
		.await
		.unwrap();
	assert_eq!(personal.unwrap().id, alice.id);
	assert_eq!(tenant.unwrap().id, shared.id);
	let (other_personal, other_tenant) = policy
		.registered_clients("beta", "alice", "https://auth.example.test/issuer")
		.await
		.unwrap();
	assert!(other_personal.is_none());
	assert!(other_tenant.is_none());

	let list = service.list("alpha", "alice").await.unwrap();
	assert_eq!(list.len(), 2);
	assert!(!list.iter().any(|row| row.id == bob.id));
	let public = serde_json::to_string(&list).unwrap();
	assert!(!public.contains("canary"));
	assert!(!public.contains("secret_resource"));
	assert!(
		service
			.delete("alpha", Some("alice"), bob.id)
			.await
			.is_err()
	);
	let row = service.get("alpha", "alice", alice.id).await.unwrap();
	let payload = f
		.runtime
		.store
		.provider_key_material_reader
		.as_ref()
		.unwrap()
		.read(
			"alpha",
			row.secret_resource.as_deref().unwrap(),
			row.pinned_version.as_deref().unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(payload.expose_secret(), "alice-canary");
	let resource = row.secret_resource.unwrap();
	let mut rotation = input(Ownership::Me, Some("new-canary"));
	rotation.expected_revision = alice.revision;
	let rotated = service
		.put("alpha", "alice", Some(alice.id), rotation)
		.await
		.unwrap();
	assert_eq!(rotated.revision, alice.revision + 1);
	assert_eq!(store.versions("alpha", &resource).await.unwrap().len(), 1);
	service.retire_subject("alpha", "alice").await.unwrap();
	assert!(store.versions("alpha", &resource).await.unwrap().is_empty());
	let (personal, tenant) = policy
		.registered_clients("alpha", "alice", "https://auth.example.test/issuer")
		.await
		.unwrap();
	assert!(personal.is_none());
	assert_eq!(tenant.unwrap().id, shared.id);
	assert_eq!(
		service.get("alpha", "bob", bob.id).await.unwrap().id,
		bob.id
	);
	let ceiling = service
		.put_ceiling(
			"alpha",
			"https://mcp.example.test/tools/",
			vec!["write".into(), "read".into(), "read".into()],
			0,
		)
		.await
		.unwrap();
	assert_eq!(ceiling.resource, "https://mcp.example.test/tools");
	assert_eq!(
		policy
			.scope_ceiling("alpha", "https://mcp.example.test/tools")
			.await
			.unwrap(),
		Some(vec!["read".into(), "write".into()])
	);
	assert_eq!(
		policy
			.scope_ceiling("beta", "https://mcp.example.test/tools")
			.await
			.unwrap(),
		None
	);
	assert!(
		service
			.put_ceiling("alpha", &ceiling.resource, vec![], 0)
			.await
			.is_err()
	);
	let updated = service
		.put_ceiling(
			"alpha",
			&ceiling.resource,
			vec!["read".into()],
			ceiling.revision,
		)
		.await
		.unwrap();
	assert_eq!(updated.revision, ceiling.revision + 1);
	assert_eq!(
		policy
			.scope_ceiling("alpha", &ceiling.resource)
			.await
			.unwrap(),
		Some(vec!["read".into()])
	);
	let other = service
		.put_ceiling(
			"alpha",
			"https://mcp.example.test/other",
			vec!["write".into()],
			0,
		)
		.await
		.unwrap();
	service
		.delete_ceiling("alpha", &ceiling.resource)
		.await
		.unwrap();
	assert_eq!(
		policy
			.scope_ceiling("alpha", &ceiling.resource)
			.await
			.unwrap(),
		None
	);
	assert_eq!(
		policy
			.scope_ceiling("alpha", &other.resource)
			.await
			.unwrap(),
		Some(vec!["write".into()])
	);
	service.retire_tenant("alpha").await.unwrap();
	personal_credentials::assert_present(store.as_ref(), "beta", &unrelated, true).await;
	assert!(service.list("alpha", "bob").await.unwrap().is_empty());
	assert!(
		store
			.versions("alpha", &store.oauth_client_resource(shared.id))
			.await
			.unwrap()
			.is_empty()
	);
}

#[rstest]
#[timeout(Duration::from_secs(600))]
#[tokio::test]
async fn oauth_client_http_checks_personal_and_tenant_actions_and_ceiling_encoding(
	#[future] endpoint: EndpointFixture,
) {
	let mut f = endpoint.await;
	f.context.set_singleton(f.runtime.clone());
	endpoint_fixtures::tenant_scope(&f.runtime.store, "alpha").await;
	endpoint_fixtures::replace_policy(&f.runtime.store, "alpha", json!({
		"tenant":"alpha", "subjects":{"alice":{"kind":"user","roles":["member"]}, "bob":{"kind":"user","roles":["member"]}},
		"roles":{"member":{}}, "policies":[{"id":"use", "effect":"allow", "subjects":{"roles":["member"]}, "actions":["secret.use"], "resources":{"kinds":["secret"]}}]
	})).await;
	let client = reqwest::Client::new();
	let authorization = aidash_server::authorization::Authorization {
		pool: f.runtime.store.pool.clone(),
	};
	let alice = authorization
		.issue_credential("alpha", "alice", 3600, "admin")
		.await
		.unwrap()
		.token;
	let bob = authorization
		.issue_credential("alpha", "bob", 3600, "admin")
		.await
		.unwrap()
		.token;
	let url = format!("{}/api/tenants/alpha/oauth-clients", f.server.url);
	let unavailable = client
		.put(&url)
		.bearer_auth(&alice)
		.json(&json!({"owner":"me", "issuer":"https://auth.example.test", "client_id":"personal", "client_secret":"unsupported-canary"}))
		.send()
		.await
		.unwrap();
	assert_eq!(unavailable.status(), 503);
	let store = personal_credentials::configure(&mut f.runtime).await;
	f.context.set_singleton(f.runtime.clone());
	let body = json!({"owner":"me", "issuer":"https://AUTH.example.test/", "client_id":"personal", "client_secret":"http-client-canary"});
	let result = client
		.put(&url)
		.bearer_auth(&alice)
		.json(&body)
		.send()
		.await
		.unwrap();
	assert_eq!(result.status(), 200);
	let row: serde_json::Value = result.json().await.unwrap();
	assert_eq!(row["owner"], "me");
	assert_eq!(row["issuer"], "https://auth.example.test");
	let id = row["id"].as_str().unwrap();
	let service = personal_credentials::clients(&f.runtime);
	let client_id = row["id"].as_str().unwrap().parse().unwrap();
	let original = service.get("alpha", "alice", client_id).await.unwrap();
	let resource = original.secret_resource.as_deref().unwrap();
	let versions = store.versions("alpha", resource).await.unwrap();
	for replacement in [None, Some("replacement-canary")] {
		let rejected = client.put(format!("{url}/{id}"))
			.bearer_auth(&alice)
			.json(&json!({"owner":"me", "issuer":"https://other.example.test", "client_id":"retargeted", "client_secret":replacement, "expected_revision":row["revision"]}))
			.send().await.unwrap();
		assert_eq!(rejected.status(), 409);
		assert_eq!(
			rejected.json::<serde_json::Value>().await.unwrap()["error"],
			"oauth_client_issuer_immutable"
		);
		let unchanged = service.get("alpha", "alice", client_id).await.unwrap();
		assert_eq!(unchanged.issuer, original.issuer);
		assert_eq!(unchanged.client_id, original.client_id);
		assert_eq!(unchanged.revision, original.revision);
		assert_eq!(unchanged.secret_resource, original.secret_resource);
		assert_eq!(unchanged.pinned_version, original.pinned_version);
		assert_eq!(store.versions("alpha", resource).await.unwrap(), versions);
		let payload = f
			.runtime
			.store
			.provider_key_material_reader
			.as_ref()
			.unwrap()
			.read(
				"alpha",
				resource,
				original.pinned_version.as_deref().unwrap(),
			)
			.await
			.unwrap();
		assert_eq!(payload.expose_secret(), "http-client-canary");
	}
	personal_credentials::clients(&f.runtime)
		.put("alpha", "admin", None, input(Ownership::Tenant, None))
		.await
		.unwrap();
	let list = client.get(&url).bearer_auth(&alice).send().await.unwrap();
	assert_eq!(list.status(), 200);
	let list: serde_json::Value = list.json().await.unwrap();
	assert_eq!(list.as_array().unwrap().len(), 1);
	assert_eq!(list[0]["id"], id);

	assert_eq!(
		client
			.get(format!("{url}/{id}"))
			.bearer_auth(&bob)
			.send()
			.await
			.unwrap()
			.status(),
		404
	);
	assert_eq!(
		client
			.delete(format!("{url}/{id}"))
			.bearer_auth(&bob)
			.send()
			.await
			.unwrap()
			.status(),
		404
	);
	assert_eq!(
		client
			.put(&url)
			.bearer_auth(&alice)
			.json(
				&json!({"owner":"tenant", "issuer":"https://auth.example.test", "client_id":"shared"})
			)
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	let resource = URL_SAFE_NO_PAD.encode("https://mcp.example.test");
	let ceilings = format!(
		"{}/api/tenants/alpha/connection-scope-ceilings/{resource}",
		f.server.url
	);
	assert_eq!(
		client
			.put(&ceilings)
			.bearer_auth(&alice)
			.json(&json!({"scopes":["read"]}))
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	assert_eq!(
		client
			.get(format!(
				"{}/api/tenants/alpha/connection-scope-ceilings/not_base64",
				f.server.url
			))
			.bearer_auth(&alice)
			.send()
			.await
			.unwrap()
			.status(),
		400
	);
	let secrets = format!("{}/api/tenants/alpha/secrets/api", f.server.url);
	let secret_service = f.runtime.store.tenant_secrets.as_ref().unwrap();
	for owner in [None, Some("bob")] {
		secret_service
			.put_owned(
				"alpha",
				owner,
				"api",
				aidash_application::tenant_secrets::Put {
					expected_revision: 0,
					value: Some("other-owner-canary".into()),
					hosts: Some(vec!["api.example.test".into()]),
				},
				"fixture",
			)
			.await
			.unwrap();
	}

	let result = client.put(&secrets).bearer_auth(&alice).json(&json!({"owner":"me","expected_revision":0,"value":"alice-personal-canary","hosts":["api.example.test"]})).send().await.unwrap();
	assert_eq!(result.status(), 200);
	let secret: serde_json::Value = result.json().await.unwrap();
	assert_eq!(secret["owner"], "me");
	let visible = client
		.get(format!("{}/api/tenants/alpha/secrets", f.server.url))
		.bearer_auth(&alice)
		.send()
		.await
		.unwrap();
	assert_eq!(visible.status(), 200);
	let visible: serde_json::Value = visible.json().await.unwrap();
	assert_eq!(visible.as_array().unwrap().len(), 2);
	assert!(
		visible
			.as_array()
			.unwrap()
			.iter()
			.any(|row| row["owner"] == "tenant")
	);
	assert!(
		visible
			.as_array()
			.unwrap()
			.iter()
			.any(|row| row["id"] == secret["id"])
	);

	let bob_secret = client
		.get(format!("{secrets}?owner=me"))
		.bearer_auth(&bob)
		.send()
		.await
		.unwrap();
	assert_eq!(bob_secret.status(), 200);
	let bob_secret: serde_json::Value = bob_secret.json().await.unwrap();
	assert_eq!(bob_secret["owner"], "me");
	assert_ne!(bob_secret["id"], secret["id"]);
	let shared = client
		.get(&secrets)
		.bearer_auth(&alice)
		.send()
		.await
		.unwrap();
	assert_eq!(shared.status(), 200);
	let shared: serde_json::Value = shared.json().await.unwrap();
	assert_eq!(shared["owner"], "tenant");
	assert_eq!(shared["hosts"], json!(["api.example.test"]));
	assert!(shared["fingerprint"].is_string());
	assert_ne!(shared["id"], secret["id"]);
	assert_eq!(
		client
			.delete(&secrets)
			.bearer_auth(&alice)
			.json(&json!({"expected_revision":shared["revision"]}))
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	assert_eq!(
		client
			.put(&secrets)
			.bearer_auth(&alice)
			.json(
				&json!({"expected_revision":0,"value":"shared-canary","hosts":["api.example.test"]})
			)
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	assert_eq!(
		client
			.delete(&secrets)
			.bearer_auth(&alice)
			.json(&json!({"owner":"me", "expected_revision":secret["revision"]}))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	assert_eq!(
		client
			.delete(format!("{url}/{id}"))
			.bearer_auth(&alice)
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
}

#[rstest]
#[timeout(Duration::from_secs(600))]
#[tokio::test]
async fn scoped_client_manager_uses_the_stored_issuer_for_all_record_actions(
	#[future] endpoint: EndpointFixture,
) {
	let mut f = endpoint.await;
	endpoint_fixtures::tenant_scope(&f.runtime.store, "alpha").await;
	endpoint_fixtures::replace_policy(&f.runtime.store, "alpha", json!({
		"tenant":"alpha", "subjects":{"alice":{"kind":"user","roles":["manager"]}},
		"roles":{"manager":{}}, "policies":[
			{"id":"collection", "effect":"allow", "subjects":{"roles":["manager"]}, "actions":["secret.use"], "resources":{"kinds":["secret"]}},
			{"id":"scoped-manage", "effect":"allow", "subjects":{"roles":["manager"]}, "actions":["secret.manage"], "resources":{"kinds":["secret"]},
			 "condition":{"op":"eq","left":{"source":"resource","path":"/name"},"right":{"source":"literal","value":"https://allowed.example.test"}}}
		]
	})).await;
	let store = personal_credentials::configure(&mut f.runtime).await;
	f.context.set_singleton(f.runtime.clone());
	let service = personal_credentials::clients(&f.runtime);
	let mut allowed = input(Ownership::Tenant, None);
	allowed.issuer = "https://allowed.example.test".into();
	let allowed = service.put("alpha", "admin", None, allowed).await.unwrap();
	let mut blocked = input(Ownership::Tenant, Some("blocked-client-canary"));
	blocked.issuer = "https://blocked.example.test".into();
	let blocked = service.put("alpha", "admin", None, blocked).await.unwrap();
	let original = service.get("alpha", "alice", blocked.id).await.unwrap();
	let resource = original.secret_resource.as_deref().unwrap();
	let versions = store.versions("alpha", resource).await.unwrap();
	let authorization = aidash_server::authorization::Authorization {
		pool: f.runtime.store.pool.clone(),
	};
	let token = authorization
		.issue_credential("alpha", "alice", 3600, "admin")
		.await
		.unwrap()
		.token;
	let client = reqwest::Client::new();
	let url = format!("{}/api/tenants/alpha/oauth-clients", f.server.url);
	assert_eq!(
		client
			.get(format!("{url}/{}", allowed.id))
			.bearer_auth(&token)
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	assert_eq!(
		client
			.get(format!("{url}/{}", blocked.id))
			.bearer_auth(&token)
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	let rejected = client.put(format!("{url}/{}", blocked.id)).bearer_auth(&token)
		.json(&json!({"owner":"tenant", "issuer":allowed.issuer, "client_id":"retargeted", "expected_revision":blocked.revision}))
		.send().await.unwrap();
	let put_status = rejected.status().as_u16();
	assert_eq!(
		client
			.delete(format!("{url}/{}", blocked.id))
			.bearer_auth(&token)
			.send()
			.await
			.unwrap()
			.status(),
		403
	);
	let list = client.get(&url).bearer_auth(&token).send().await.unwrap();
	assert_eq!(list.status(), 200);
	let list: serde_json::Value = list.json().await.unwrap();
	let ids: Vec<_> = list
		.as_array()
		.unwrap()
		.iter()
		.map(|row| row["id"].as_str().unwrap().to_owned())
		.collect();
	assert_eq!((put_status, ids), (403, vec![allowed.id.to_string()]));
	let unchanged = service.get("alpha", "alice", blocked.id).await.unwrap();
	assert_eq!(unchanged.issuer, original.issuer);
	assert_eq!(unchanged.client_id, original.client_id);
	assert_eq!(unchanged.revision, original.revision);
	assert_eq!(unchanged.secret_resource, original.secret_resource);
	assert_eq!(unchanged.pinned_version, original.pinned_version);
	assert_eq!(store.versions("alpha", resource).await.unwrap(), versions);
	let payload = f
		.runtime
		.store
		.provider_key_material_reader
		.as_ref()
		.unwrap()
		.read(
			"alpha",
			resource,
			original.pinned_version.as_deref().unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(payload.expose_secret(), "blocked-client-canary");
}
