//! Shared lifecycle fixture: real encrypted Store and application services.
use aidash_application::{
	oauth_clients::{Put, Service},
	provider_credentials::Store,
	tenant_secrets::{Ownership, Put as SecretPut},
};
use aidash_server::federation::Federation;
use serde_json::json;
use std::sync::Arc;

pub async fn configure(f: &mut Federation) -> Arc<dyn Store> {
	let directory = tempfile::tempdir().unwrap();
	let master = directory.path().join("master");
	let fingerprint = directory.path().join("fingerprint");
	std::fs::write(&master, "66".repeat(32)).unwrap();
	std::fs::write(&fingerprint, "personal-secret-fixture-fingerprint-key").unwrap();
	let settings = serde_json::from_value(json!({"fingerprint_key":{"file":fingerprint}, "store":{"kind":"postgres", "master_key":{"file":master}}})).unwrap();
	aidash_server::bootstrap::configure_provider_credentials(&mut f.store, &settings)
		.await
		.unwrap();
	aidash_server::bootstrap::configure_tenant_secrets(
		&mut f.store,
		&settings,
		aidash_server::bootstrap::ProcessRole::Server,
	)
	.unwrap();
	f.store.provider_credentials.as_ref().unwrap().store.clone()
}
pub fn clients(f: &Federation) -> Service {
	Service {
		repository: Arc::new(
			aidash_server::apps::identity::repositories::oauth_clients::NativeRepository {
				pool: f.store.control_pool.clone(),
			},
		),
		store: f
			.store
			.provider_credentials
			.as_ref()
			.map(|credentials| credentials.store.clone()),
	}
}
pub struct Contents {
	pub secret_resource: String,
	pub client_resource: String,
}
pub async fn seed(f: &Federation, tenant: &str, subject: &str) -> Contents {
	let secrets = f.store.tenant_secrets.as_ref().unwrap();
	let secret = secrets
		.put_owned(
			tenant,
			Some(subject),
			"personal-api",
			SecretPut {
				expected_revision: 0,
				value: Some("personal-lifecycle-canary".into()),
				hosts: Some(vec!["api.example.test".into()]),
			},
			subject,
		)
		.await
		.unwrap();
	let client = clients(f)
		.put(
			tenant,
			subject,
			None,
			Put {
				owner: Ownership::Me,
				issuer: "https://auth.example.test".into(),
				client_id: "client".into(),
				client_secret: Some("client-lifecycle-canary".into()),
				expected_revision: 0,
			},
		)
		.await
		.unwrap();
	let store = &f.store.provider_credentials.as_ref().unwrap().store;
	Contents {
		secret_resource: store.secret_resource(secret.id),
		client_resource: store.oauth_client_resource(client.id),
	}
}
pub async fn assert_present(store: &dyn Store, tenant: &str, contents: &Contents, present: bool) {
	assert_eq!(
		!store
			.versions(tenant, &contents.secret_resource)
			.await
			.unwrap()
			.is_empty(),
		present,
		"personal secret versions"
	);
	assert_eq!(
		!store
			.versions(tenant, &contents.client_resource)
			.await
			.unwrap()
			.is_empty(),
		present,
		"OAuth Client secret versions"
	);
}
