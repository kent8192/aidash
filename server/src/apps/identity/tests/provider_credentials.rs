//! Production-router and PostgreSQL proof of Provider Credential isolation and secrecy.
#[path = "../../execution/tests/support/endpoint.rs"]
mod endpoint_fixtures;
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
use aidash_application::{
	Result,
	provider_credentials::{KeyValidator, Service, Store, Validation},
};
use aidash_domain::provider_credentials::Provider;
use aidash_server::apps::identity::repositories::provider_credentials::NativeRepository;
use async_trait::async_trait;
use endpoint_fixtures::{EndpointFixture, assert_json, endpoint};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use rstest::rstest;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, OnceLock};
use uuid::Uuid;

struct NoEnvironment;
impl aidash_application::ports::Credentials for NoEnvironment {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("Tenant access must not read environment keys")
	}
}

static LOG: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();
struct LogWriter(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogWriter {
	fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
		self.0.lock().unwrap().extend_from_slice(b);
		Ok(b.len())
	}
	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}
#[ctor::ctor]
fn capture_logs() {
	let log = LOG.get_or_init(Default::default).clone();
	let _ = tracing_subscriber::fmt()
		.with_ansi(false)
		.with_writer(move || LogWriter(log.clone()))
		.try_init();
}
#[derive(Default)]
struct FakeStore(Mutex<std::collections::BTreeMap<String, Vec<String>>>);
#[async_trait]
impl Store for FakeStore {
	fn resource(&self, id: Uuid) -> String {
		format!("fake/{id}")
	}
	async fn create(&self, _tenant: &str, id: Uuid) -> Result<()> {
		self.0.lock().unwrap().insert(self.resource(id), vec![]);
		Ok(())
	}
	async fn add_version(&self, _tenant: &str, resource: &str, _: &SecretString) -> Result<String> {
		let mut values = self.0.lock().unwrap();
		let versions = values.get_mut(resource).unwrap();
		let name = format!("{resource}/versions/{}", versions.len() + 1);
		versions.push(name.clone());
		Ok(name)
	}
	async fn versions(&self, resource: &str) -> Result<Vec<String>> {
		Ok(self
			.0
			.lock()
			.unwrap()
			.get(resource)
			.cloned()
			.unwrap_or_default())
	}
	async fn disable(&self, _: &str) -> Result<()> {
		Ok(())
	}
	async fn destroy(&self, _: &str) -> Result<()> {
		Ok(())
	}
	async fn delete(&self, resource: &str) -> Result<()> {
		self.0.lock().unwrap().remove(resource);
		Ok(())
	}
}
struct Validator;
#[async_trait]
impl KeyValidator for Validator {
	async fn validate(&self, _: Provider, _: &SecretString) -> Result<Validation> {
		Ok(Validation {
			warnings: vec!["No spending limit is configured".into()],
		})
	}
}

async fn all_database_text(f: &EndpointFixture) -> String {
	let mut tx = f.database.connection.begin().await.unwrap();
	let (sql, values) = Query::select()
		.column(Alias::new("tablename"))
		.from(Alias::new("pg_tables"))
		.and_where(Expr::col("schemaname").eq(Expr::value("public")))
		.build(PostgresQueryBuilder);
	let tables = tx.fetch_all(&sql, convert_values(values)).await.unwrap();
	let mut result = String::new();
	for table in tables {
		let name: String = table.get("tablename").unwrap();
		let (sql, values) = Query::select()
			.expr_as(
				Expr::col(Alias::new("record")).cast_as(Alias::new("text")),
				Alias::new("document"),
			)
			.from_as(Alias::new(&name), Alias::new("record"))
			.build(PostgresQueryBuilder);
		for row in tx.fetch_all(&sql, convert_values(values)).await.unwrap() {
			result.push_str(&row.get::<String>("document").unwrap());
		}
	}
	result
}

#[rstest]
#[tokio::test]
async fn credential_list_paginates_authorized_rows_beyond_the_first_page(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_domain::provider_credentials::{ProviderCredential, State};
	use aidash_server::apps::identity::models::AuthorizationDecision;
	use reinhardt::db::orm::Model;
	async fn decisions(f: &EndpointFixture) -> Vec<AuthorizationDecision> {
		let mut tx = f.database.connection.begin().await.unwrap();
		let rows = AuthorizationDecision::objects()
			.filter(AuthorizationDecision::field_tenant().eq("alpha"))
			.filter(AuthorizationDecision::field_subject().eq("alice"))
			.filter(AuthorizationDecision::field_action().eq("provider_credential.read"))
			.order_by(&["sequence"])
			.all_with_executor(tx.as_mut())
			.await
			.unwrap();
		tx.commit().await.unwrap();
		rows
	}
	let mut f = endpoint.await;
	let ids: Vec<_> = (0..203).map(|_| Uuid::now_v7()).collect();
	let bundle = json!({
		"tenant":"alpha",
		"subjects":{"alice":{"kind":"user"}},
		"policies":[{
			"id":"selected-credentials", "effect":"allow",
			"subjects":{"ids":["alice"]},
			"actions":["provider_credential.read"],
			"resources":{"kinds":["provider_credential"],"ids":&ids[200..]}
		}]
	});
	assert_json(
		f.operator
			.post(
				"/api/authorization/alpha",
				&json!({"expected_revision":0,"bundle":bundle}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let service = Arc::new(Service {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
			node: f.runtime.store.node_id.clone(),
		}),
		store: Arc::new(FakeStore::default()),
		validator: Arc::new(Validator),
		fingerprint_key: "test-fingerprint-key-that-is-at-least-32-bytes".into(),
		max_per_tenant: ids.len(),
	});
	// Seed metadata in chronological order: the first 200 records are denied.
	let mut scope = service.repository.begin("alpha").await.unwrap();
	let created_at = chrono::Utc::now();
	for (index, id) in ids.iter().enumerate() {
		scope
			.insert(&ProviderCredential {
				id: *id,
				tenant: "alpha".into(),
				provider: Provider::Openrouter,
				base_url: Provider::Openrouter.base_url().into(),
				secret_resource: format!("fake/{id}"),
				pinned_version: Some(format!("fake/{id}/versions/1")),
				fingerprint: "0123456789abcdef".into(),
				last4: "test".into(),
				state: State::Active,
				created_at: created_at + chrono::Duration::milliseconds(index as i64),
				rotated_at: None,
				revoked_at: None,
				revision: 1,
			})
			.await
			.unwrap();
	}
	scope.commit().await.unwrap();
	f.runtime.store.provider_credentials = Some(service);
	f.context.set_singleton(f.runtime.clone());
	let token = assert_json(
		f.operator
			.post(
				"/api/authorization/alpha/credentials",
				&json!({"subject":"alice","expires_in_seconds":3600}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let subject = reinhardt::test::fixtures::api_client_from_url(&f.server.url);
	subject
		.set_header(
			"Authorization",
			&format!("Bearer {}", token["token"].as_str().unwrap()),
		)
		.await
		.unwrap();

	// The default dashboard page reaches allowed rows after a full denied batch;
	// offset and limit then refer to the authorized set, without gaps or duplicates.
	for (query, expected) in [
		("", &ids[200..]),
		("?offset=0&limit=2", &ids[200..202]),
		("?offset=2&limit=2", &ids[202..]),
	] {
		let rows = assert_json(
			subject
				.get(&format!("/api/tenants/alpha/provider-credentials{query}"))
				.await
				.unwrap(),
			200,
		);
		let actual: Vec<_> = rows
			.as_array()
			.unwrap()
			.iter()
			.map(|row| Uuid::parse_str(row["id"].as_str().unwrap()).unwrap())
			.collect();
		assert_eq!(actual, expected, "page {query}");
	}
	// An offset beyond the visible set still finalizes exactly one allow/deny
	// audit for every evaluated record before returning the empty page.
	let before = decisions(&f).await.len();
	let empty = assert_json(
		subject
			.get("/api/tenants/alpha/provider-credentials?offset=3&limit=2")
			.await
			.unwrap(),
		200,
	);
	assert_eq!(empty, json!([]));
	let audits = decisions(&f).await;
	let page_audits = &audits[before..];
	assert_eq!(page_audits.len(), ids.len());
	for (index, (decision, id)) in page_audits.iter().zip(&ids).enumerate() {
		assert_eq!(decision.resource_id, id.to_string());
		assert_eq!(decision.decision.0["allowed"], index >= 200);
	}
	let operator_rows = assert_json(
		f.operator
			.get("/api/tenants/alpha/provider-credentials?offset=201&limit=2")
			.await
			.unwrap(),
		200,
	);
	assert_eq!(operator_rows[0]["id"], ids[201].to_string());
	assert_eq!(operator_rows[1]["id"], ids[202].to_string());
	assert_eq!(operator_rows.as_array().unwrap().len(), 2);
}

#[rstest]
#[tokio::test]
async fn lifecycle_api_uses_live_tenant_policy_and_never_persists_or_returns_key_material(
	#[future] endpoint: EndpointFixture,
) {
	let mut f = endpoint.await;
	let schema = serde_json::to_value(schemars::schema_for!(
		aidash_server::apps::identity::serializers::provider_credentials::BindingUpdate
	))
	.unwrap();
	assert!(
		schema["required"]
			.as_array()
			.unwrap()
			.contains(&json!("provider_credential_id"))
	);
	assert_eq!(
		schema["properties"]["provider_credential_id"]["type"],
		json!(["string", "null"])
	);

	let disabled = f
		.operator
		.get("/api/tenants/alpha/provider-credentials")
		.await
		.unwrap();
	assert_eq!(disabled.status_code(), 404);
	for tenant in ["alpha", "beta"] {
		let bundle = json!({"tenant":tenant,"subjects":{"alice":{"kind":"user"}},"policies":[{"id":"provider-access","effect":"allow","subjects":{"ids":["alice"]},"actions":["provider_credential.create","provider_credential.read","provider_credential.rotate","provider_credential.revoke","provider_credential.delete","provider_credential_binding.read","provider_credential_binding.update"],"resources":{"kinds":["provider_credential","provider_credential_binding"]}}]});
		assert_json(
			f.operator
				.post(
					&format!("/api/authorization/{tenant}"),
					&json!({"expected_revision":0,"bundle":bundle}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
	}
	let service = Arc::new(Service {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
			node: f.runtime.store.node_id.clone(),
		}),
		store: Arc::new(FakeStore::default()),
		validator: Arc::new(Validator),
		fingerprint_key: "test-fingerprint-key-that-is-at-least-32-bytes".into(),
		max_per_tenant: 2,
	});
	f.runtime.store.provider_credentials = Some(service.clone());
	f.runtime.registry = f.runtime.registry.clone().with_provider_credentials(true);
	f.context.set_singleton(f.runtime.clone());
	let management = aidash_server::apps::identity::services::provider_credentials::Management {
		runtime: f.runtime.clone(),
	};
	let canary = "canary-provider-key-do-not-persist-a7f91234";
	// Public Key Material ingestion is intentionally absent, even with a Store.
	for path in [
		"/api/tenants/alpha/provider-credentials".to_owned(),
		format!(
			"/api/tenants/alpha/provider-credentials/{}/rotate",
			Uuid::now_v7()
		),
	] {
		let response = f
			.operator
			.post(
				&path,
				&json!({"provider":"openrouter", "key_material":canary, "expected_revision":0}),
				"json",
			)
			.await
			.unwrap();
		assert!(matches!(response.status_code(), 404 | 405));
		assert!(!response.text().contains(canary));
	}
	let mut empty = service.repository.begin("alpha").await.unwrap();
	assert_eq!(empty.count().await.unwrap(), 0);
	empty.commit().await.unwrap();
	let created = serde_json::to_value(
		management
			.create(
				aidash_server::authorization::identity::Actor::Operator,
				"alpha".into(),
				Provider::Openrouter,
				canary.into(),
			)
			.await
			.unwrap(),
	)
	.unwrap();
	assert!(!created.to_string().contains(canary));
	assert!(!created.to_string().contains("secret_resource"));
	assert!(!created.to_string().contains("pinned_version"));
	assert_eq!(created["warnings"].as_array().unwrap().len(), 1);
	let id = created["provider_credential"]["id"].as_str().unwrap();
	let revision = created["provider_credential"]["revision"].as_i64().unwrap();
	let token = assert_json(
		f.operator
			.post(
				"/api/authorization/beta/credentials",
				&json!({"subject":"alice","expires_in_seconds":3600}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let subject = reinhardt::test::fixtures::api_client_from_url(&f.server.url);
	subject
		.set_header(
			"Authorization",
			&format!("Bearer {}", token["token"].as_str().unwrap()),
		)
		.await
		.unwrap();
	assert_eq!(
		subject
			.get(&format!("/api/tenants/alpha/provider-credentials/{id}"))
			.await
			.unwrap()
			.status_code(),
		403
	);
	assert_eq!(
		f.operator
			.get(&format!("/api/tenants/beta/provider-credentials/{id}"))
			.await
			.unwrap()
			.status_code(),
		404
	);
	let alpha_token = assert_json(
		f.operator
			.post(
				"/api/authorization/alpha/credentials",
				&json!({"subject":"alice","expires_in_seconds":3600}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let alpha_subject = reinhardt::test::fixtures::api_client_from_url(&f.server.url);
	alpha_subject
		.set_header(
			"Authorization",
			&format!("Bearer {}", alpha_token["token"].as_str().unwrap()),
		)
		.await
		.unwrap();
	let disclosed = assert_json(
		alpha_subject
			.get(&format!("/api/tenants/alpha/provider-credentials/{id}"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(disclosed["id"], id);
	assert!(!disclosed.to_string().contains(canary));
	let second = serde_json::to_value(
		management
			.create(
				aidash_server::authorization::identity::Actor::Operator,
				"alpha".into(),
				Provider::Openrouter,
				canary.into(),
			)
			.await
			.unwrap(),
	)
	.unwrap();
	assert!(!second.to_string().contains(canary));
	assert!(matches!(
		management
			.create(
				aidash_server::authorization::identity::Actor::Operator,
				"alpha".into(),
				Provider::Openrouter,
				canary.into(),
			)
			.await,
		Err(aidash_server::Error::Conflict(_))
	));
	let malformed = reqwest::Client::new()
		.put(format!(
			"{}/api/tenants/alpha/provider-credential-bindings/openrouter",
			f.server.url
		))
		.bearer_auth(&f.runtime.config.api_token)
		.json(&json!({"expected_revision":0}))
		.send()
		.await
		.unwrap();
	assert_eq!(malformed.status().as_u16(), 422);
	assert_eq!(malformed.headers()["cache-control"], "no-store");
	let bound = assert_json(
		f.operator
			.put(
				"/api/tenants/alpha/provider-credential-bindings/openrouter",
				&json!({"provider_credential_id":id,"expected_revision":0}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(bound["revision"], 1);
	let response = f
		.operator
		.put(
			"/api/tenants/alpha/provider-credential-bindings/openrouter",
			&json!({"provider_credential_id":id,"expected_revision":0}),
			"json",
		)
		.await
		.unwrap();
	assert_eq!(response.status_code(), 409);
	let rotated = serde_json::to_value(
		management
			.rotate(
				aidash_server::authorization::identity::Actor::Operator,
				"alpha".into(),
				Uuid::parse_str(id).unwrap(),
				revision,
				canary.into(),
			)
			.await
			.unwrap(),
	)
	.unwrap();
	assert!(!rotated.to_string().contains(canary));
	assert_eq!(rotated["provider_credential"]["revision"], revision + 1);
	assert_eq!(
		f.operator
			.post(
				&format!("/api/tenants/alpha/provider-credentials/{id}/revoke"),
				&json!({"expected_revision":revision}),
				"json"
			)
			.await
			.unwrap()
			.status_code(),
		409
	);
	let revoked = assert_json(
		f.operator
			.post(
				&format!("/api/tenants/alpha/provider-credentials/{id}/revoke"),
				&json!({"expected_revision":revision+1}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(revoked["state"], "revoked");
	let response = reqwest::Client::new()
		.delete(format!(
			"{}/api/tenants/alpha/provider-credentials/{id}",
			f.server.url
		))
		.bearer_auth(&f.runtime.config.api_token)
		.json(&json!({"expected_revision":revision+2}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status().as_u16(), 409);
	assert_eq!(response.headers()["cache-control"], "no-store");
	let unbound = assert_json(
		f.operator
			.put(
				"/api/tenants/alpha/provider-credential-bindings/openrouter",
				&json!({"provider_credential_id":null,"expected_revision":1}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert!(unbound["provider_credential_id"].is_null());
	assert_eq!(unbound["revision"], 2);
	let response = reqwest::Client::new()
		.delete(format!(
			"{}/api/tenants/alpha/provider-credentials/{id}",
			f.server.url
		))
		.bearer_auth(&f.runtime.config.api_token)
		.json(&json!({"expected_revision":revision+2}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status().as_u16(), 200);
	assert_eq!(response.json::<Value>().await.unwrap()["state"], "deleted");
	let own_events = assert_json(alpha_subject.get("/api/events?after=0").await.unwrap(), 200);
	assert!(
		own_events
			.to_string()
			.contains("provider_credential.created")
	);
	assert!(
		own_events
			.to_string()
			.contains("provider_credential_binding.updated")
	);
	assert!(!own_events.to_string().contains(canary));
	let other_events = assert_json(subject.get("/api/events?after=0").await.unwrap(), 200);
	assert!(!other_events.to_string().contains(id));
	let database = all_database_text(&f).await;
	assert!(
		!database.contains(canary),
		"Key Material appeared in PostgreSQL"
	);
	let logs = String::from_utf8(LOG.get().unwrap().lock().unwrap().clone()).unwrap();
	assert!(!logs.contains(canary), "Key Material appeared in logs");
	assert!(database.contains("provider_credential.created"));
	assert!(database.contains("provider_credential.rotated"));
	assert!(database.contains("provider_credential.revoked"));
	assert!(database.contains("authenticated operator authority"));
}

#[rstest]
#[tokio::test]
async fn admission_pins_local_tenant_id_and_reloads_state_without_environment_fallback(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_application::provider_access::{Context, ProviderAccess, Source};
	use aidash_server::apps::identity::{
		models::provider_credentials::RunProviderCredential as Pin,
		repositories::provider_credentials::{AdmittedAccess, admit},
	};
	use reinhardt::db::orm::Model;
	let mut f = endpoint.await;
	let service = Arc::new(Service {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
			node: f.runtime.store.node_id.clone(),
		}),
		store: Arc::new(FakeStore::default()),
		validator: Arc::new(Validator),
		fingerprint_key: "test-fingerprint-key-that-is-at-least-32-bytes".into(),
		max_per_tenant: 20,
	});
	let model: aidash_server::registry::Entry = serde_json::from_value(json!({"id":"tenant-model","version":"1.0.0","kind":"model","name":{"en":"Tenant model"},"description":{"en":"Tenant model"},"config":{"provider":"openrouter","model_id":"test/model","endpoint":"https://openrouter.ai/api/v1","credential_env":null,"provider_credential":"openrouter","context_window":4096,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	assert!(
		f.runtime
			.registry
			.register(model.clone())
			.await
			.unwrap_err()
			.to_string()
			.contains("Store is not configured")
	);
	f.runtime.store.provider_credentials = Some(service.clone());
	f.runtime.registry = f.runtime.registry.clone().with_provider_credentials(true);
	f.runtime.registry.register(model).await.unwrap();
	let agent: aidash_server::registry::Entry = serde_json::from_value(json!({"id":"tenant-agent","version":"1.0.0","kind":"agent","name":{"en":"Tenant agent"},"description":{"en":"Tenant agent"},"config":{"schema_version":1,"bindings":[],"model":{"id":"tenant-model","version":"1.0.0"},"instructions":"Test admission","cluster":null}})).unwrap();
	struct Lookup<'a>(&'a aidash_server::registry::Registry);
	#[async_trait]
	impl aidash_application::ports::registry::DefinitionLookup for Lookup<'_> {
		async fn definition(
			&mut self,
			id: &str,
			version: &str,
		) -> Result<aidash_domain::registry::Entry> {
			self.0.get(id, version).await.map_err(Into::into)
		}
		async fn overrides(&mut self, _: &str, _: &str) -> Result<Option<Value>> {
			Ok(None)
		}
	}
	let snapshot = aidash_application::registry::bindings::resolve(
		&mut aidash_application::registry::bindings::catalog::LocalCatalog(
			aidash_application::registry::bindings::catalog::LookupCatalog {
				definitions: &mut Lookup(&f.runtime.registry),
				node: &f.runtime.config.node_id,
			},
		),
		&aidash_server::bootstrap::registry_validation().with_provider_credentials(true),
		aidash_domain::registry::bindings::QualifiedRef {
			registry_node: f.runtime.config.node_id.clone(),
			id: agent.id.clone(),
			version: agent.version.clone(),
		},
		&agent,
		false,
	)
	.await
	.unwrap();
	let a = service
		.create(
			"alpha",
			Uuid::now_v7(),
			Provider::Openrouter,
			"alpha-provider-key-1234".into(),
			"actor",
		)
		.await
		.unwrap()
		.provider_credential;
	let b = service
		.create(
			"beta",
			Uuid::now_v7(),
			Provider::Openrouter,
			"beta-provider-key-5678".into(),
			"actor",
		)
		.await
		.unwrap()
		.provider_credential;
	service
		.bind("alpha", Provider::Openrouter, a.id, 0, "actor")
		.await
		.unwrap();
	let run = Uuid::now_v7();
	let mut tx = f.database.connection.begin().await.unwrap();
	assert!(
		admit(tx.as_mut(), run, "beta", &snapshot, true)
			.await
			.unwrap_err()
			.to_string()
			.contains("binding is missing")
	);
	drop(tx);
	service
		.bind("beta", Provider::Openrouter, b.id, 0, "actor")
		.await
		.unwrap();
	let mut tx = f.database.connection.begin().await.unwrap();
	// `beta` represents the receiving local Tenant, never the sender's Tenant `alpha`.
	admit(tx.as_mut(), run, "beta", &snapshot, true)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let access = AdmittedAccess {
		store: f.runtime.store.clone(),
	};
	let source = Source::Tenant {
		provider: "openrouter".into(),
	};
	let mut context = Context {
		tenant: "beta".into(),
		run: Some(run),
		maintenance: None,
		provider_credential_id: None,
	};
	let mut maintenance = Context {
		tenant: "beta".into(),
		run: None,
		maintenance: None,
		// Supplied IDs cannot select another Tenant's record for maintenance.
		provider_credential_id: Some(a.id),
	};
	assert!(
		access
			.resolve(&maintenance, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("maintenance purpose")
	);
	maintenance.maintenance =
		Some(aidash_application::provider_access::MaintenancePurpose::MemoryIndexing);
	assert!(
		access
			.resolve(&maintenance, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("credential broker not configured")
	);
	assert!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("credential broker not configured")
	);
	context.tenant = "alpha".into();
	assert!(matches!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await,
		Err(aidash_application::Error::Forbidden)
	));
	context.tenant = "beta".into();
	let rotated = service
		.rotate(
			"beta",
			b.id,
			b.revision,
			"rotated-beta-provider-key-9012".into(),
			"actor",
		)
		.await
		.unwrap()
		.provider_credential;
	assert!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("credential broker not configured")
	);
	let replacement = service
		.create(
			"beta",
			Uuid::now_v7(),
			Provider::Openrouter,
			"replacement-beta-provider-key-4567".into(),
			"actor",
		)
		.await
		.unwrap()
		.provider_credential;
	service
		.bind("beta", Provider::Openrouter, replacement.id, 1, "actor")
		.await
		.unwrap();
	let mut tx = f.database.connection.begin().await.unwrap();
	admit(tx.as_mut(), run, "beta", &snapshot, true)
		.await
		.unwrap();
	let pins = Pin::objects()
		.filter(Pin::field_run_id().eq(run))
		.all_with_executor(tx.as_mut())
		.await
		.unwrap();
	assert_eq!(pins.len(), 1);
	assert_eq!(pins[0].tenant, "beta");
	assert_eq!(pins[0].provider_credential_id, b.id);
	tx.commit().await.unwrap();
	service
		.revoke("beta", b.id, rotated.revision, "actor")
		.await
		.unwrap();
	// Maintenance sees the current replacement, while the Run retains revoked B.
	assert!(
		access
			.resolve(&maintenance, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("credential broker not configured")
	);
	assert!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("not active")
	);
	let mut tx = f.database.connection.begin().await.unwrap();
	assert!(
		admit(tx.as_mut(), run, "beta", &snapshot, true)
			.await
			.unwrap_err()
			.to_string()
			.contains("not active")
	);
	drop(tx);
	service
		.bind("beta", Provider::Openrouter, None, 2, "actor")
		.await
		.unwrap();
	let mut tx = f.database.connection.begin().await.unwrap();
	assert!(
		admit(tx.as_mut(), Uuid::now_v7(), "beta", &snapshot, true)
			.await
			.unwrap_err()
			.to_string()
			.contains("binding is missing")
	);
	assert!(
		admit(tx.as_mut(), Uuid::now_v7(), "beta", &snapshot, false)
			.await
			.unwrap_err()
			.to_string()
			.contains("Store is not configured")
	);
}

#[rstest]
#[tokio::test]
async fn registry_migration_preserves_current_constraints_in_both_directions(
	#[future] endpoint: EndpointFixture,
) {
	let f = endpoint.await;
	let mut tx = f.database.connection.begin().await.unwrap();
	async fn definition(
		tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
		name: &str,
	) -> String {
		let (sql, values) = Query::select()
			.expr_as(
				SimpleExpr::FunctionCall(
					"pg_get_constraintdef".into_iden(),
					vec![Expr::col("oid").into()],
				),
				Alias::new("definition"),
			)
			.from(Alias::new("pg_constraint"))
			.and_where(Expr::col("conname").eq(Expr::value(name)))
			.build(PostgresQueryBuilder);
		tx.fetch_one(&sql, convert_values(values))
			.await
			.unwrap()
			.get("definition")
			.unwrap()
	}
	async fn apply_asset(
		tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
		connection: &reinhardt::db::backends::DatabaseConnection,
		sql: &str,
	) {
		use reinhardt::db::migrations::{
			Migration, MigrationDirection, Operation, PlannedStatement, ProjectState,
			plan_migration_sql,
		};
		let migration = Migration::new("provider_constraint_proof", "registry")
			.database_only(true)
			.add_operation(Operation::RunSQL {
				sql: sql.into(),
				reverse_sql: None,
			});
		let plan = plan_migration_sql(
			connection,
			&migration,
			&ProjectState::default(),
			MigrationDirection::Forward,
		)
		.await
		.unwrap();
		for statement in plan.statements {
			if let PlannedStatement::Sql(sql) = statement {
				tx.execute(&sql, vec![]).await.unwrap();
			}
		}
	}
	let current = definition(tx.as_mut(), "registry_model_config").await;
	// Unsupported JSONB CHECK DDL simulates a later migration's independent rule.
	// The forward/backward assets must reconstruct the current catalog definition.
	tx.execute(
		"ALTER TABLE registry DROP CONSTRAINT registry_model_config",
		vec![],
	)
	.await
	.unwrap();
	tx.execute(&format!("ALTER TABLE registry ADD CONSTRAINT registry_model_config CHECK (({current_expression}) AND ((metadata #>> '{{config,model_id}}') <> 'future-forbidden'))", current_expression = current.strip_prefix("CHECK (").unwrap().strip_suffix(')').unwrap()), vec![]).await.unwrap();
	let expected = definition(tx.as_mut(), "registry_model_config").await;
	let embedding = definition(tx.as_mut(), "registry_embedding_config").await;
	let agent = definition(tx.as_mut(), "registry_agent_config").await;
	apply_asset(
		tx.as_mut(),
		&f.database.connection,
		include_str!("../../../../migrations/registry/sql/backward/0016_provider_credentials.sql"),
	)
	.await;
	let reversed = definition(tx.as_mut(), "registry_model_config").await;
	assert_eq!(
		reversed,
		expected.replace("'provider_credential'::text, ", "")
	);
	assert!(reversed.contains("future-forbidden"));
	apply_asset(
		tx.as_mut(),
		&f.database.connection,
		include_str!("../../../../migrations/registry/sql/forward/0016_provider_credentials.sql"),
	)
	.await;
	assert_eq!(
		definition(tx.as_mut(), "registry_model_config").await,
		expected
	);
	assert_eq!(
		definition(tx.as_mut(), "registry_embedding_config").await,
		embedding
	);
	assert_eq!(
		definition(tx.as_mut(), "registry_agent_config").await,
		agent
	);
	tx.rollback().await.unwrap();
}

async fn postgres_store(
	f: &EndpointFixture,
	current: &str,
	retired: Vec<SecretString>,
) -> Arc<aidash_server::apps::identity::repositories::credential_store::PostgresStore> {
	Arc::new(
		aidash_server::apps::identity::repositories::credential_store::PostgresStore::new(
			f.runtime.store.control_pool.clone(),
			current.into(),
			retired,
		)
		.await
		.unwrap(),
	)
}

#[rstest]
#[tokio::test]
async fn postgres_store_service_lifecycle_resolves_pins_and_never_leaks_canary(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_application::provider_access::{
		Context, EnvironmentAccess, KeyMaterialReader, ProviderAccess, Source, TenantAccess,
	};
	let f = endpoint.await;
	let store = postgres_store(&f, &"11".repeat(32), vec![]).await;
	let service = Service {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
			node: f.runtime.store.node_id.clone(),
		}),
		store: store.clone(),
		validator: Arc::new(Validator),
		fingerprint_key: "fingerprint-test-key-at-least-32-bytes".into(),
		max_per_tenant: 20,
	};
	let canary = "canary-self-hosted-provider-key-never-persist-157!";
	let row = service
		.create(
			"alpha",
			Uuid::now_v7(),
			Provider::Openrouter,
			canary.into(),
			"actor",
		)
		.await
		.unwrap()
		.provider_credential;
	let resource = store.resource(row.id);
	let old = format!("{resource}/versions/1");
	let access = TenantAccess {
		environment: EnvironmentAccess {
			credentials: Arc::new(NoEnvironment),
		},
		repository: service.repository.clone(),
		reader: Some(store.clone()),
	};
	let context = Context {
		tenant: "alpha".into(),
		run: Some(Uuid::now_v7()),
		provider_credential_id: Some(row.id),
		..Default::default()
	};
	let source = Source::Tenant {
		provider: "openrouter".into(),
	};
	assert_eq!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap()
			.bearer
			.expose_secret(),
		canary
	);
	assert!(store.read("beta", &resource, &old).await.is_err());
	assert!(
		store
			.add_version("beta", &resource, &"other-key".into())
			.await
			.is_err()
	);
	assert!(store.create("alpha", row.id).await.is_err());
	service
		.rotate(
			"alpha",
			row.id,
			2,
			"replacement-provider-key-157".into(),
			"actor",
		)
		.await
		.unwrap();
	assert!(store.read("alpha", &resource, &old).await.is_err());
	assert_eq!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap()
			.bearer
			.expose_secret(),
		"replacement-provider-key-157"
	);
	// Scan all persisted rows while ciphertext exists, not only after deletion.
	let mut persisted = all_database_text(&f).await;
	service.revoke("alpha", row.id, 3, "actor").await.unwrap();
	assert!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.is_err()
	);
	assert!(
		store
			.read("alpha", &resource, &format!("{resource}/versions/2"))
			.await
			.is_err()
	);
	service.delete("alpha", row.id, 4, "actor").await.unwrap();
	store.disable(&old).await.unwrap();
	store.destroy(&old).await.unwrap();
	store.delete(&resource).await.unwrap();
	assert!(store.versions(&resource).await.unwrap().is_empty());
	use aidash_server::apps::identity::models::credential_store::{Resource, Version};
	use reinhardt::db::orm::Model;
	let mut tx = f.database.connection.begin().await.unwrap();
	assert!(
		Resource::objects()
			.all()
			.all_with_executor(tx.as_mut())
			.await
			.unwrap()
			.is_empty()
	);
	assert!(
		Version::objects()
			.all()
			.all_with_executor(tx.as_mut())
			.await
			.unwrap()
			.is_empty()
	);
	tx.commit().await.unwrap();
	persisted.push_str(&all_database_text(&f).await);
	persisted.push_str(&String::from_utf8(LOG.get().unwrap().lock().unwrap().clone()).unwrap());
	use base64::Engine;
	for value in [
		canary.to_owned(),
		canary.bytes().map(|b| format!("{b:02x}")).collect(),
		base64::engine::general_purpose::STANDARD.encode(canary),
	] {
		assert!(!persisted.contains(&value), "canary leaked");
	}
}

#[rstest]
#[case::version("alpha", true)]
#[case::record("alpha", false)]
#[case::tenant("beta", false)]
#[tokio::test]
async fn postgres_store_rejects_ciphertext_copied_to_another_identity(
	#[future] endpoint: EndpointFixture,
	#[case] destination_tenant: &str,
	#[case] same_resource: bool,
) {
	use aidash_application::provider_access::KeyMaterialReader;
	use aidash_server::apps::identity::models::credential_store::Version;
	use reinhardt::db::orm::Model;
	let f = endpoint.await;
	let store = postgres_store(&f, &"22".repeat(32), vec![]).await;
	let a = Uuid::now_v7();
	store.create("alpha", a).await.unwrap();
	let a_resource = store.resource(a);
	let source = store
		.add_version("alpha", &a_resource, &"source-provider-key".into())
		.await
		.unwrap();
	let resource = if same_resource || destination_tenant == "beta" {
		a_resource.clone()
	} else {
		let b = Uuid::now_v7();
		store.create(destination_tenant, b).await.unwrap();
		store.resource(b)
	};
	let dest = if destination_tenant == "beta" {
		// Isolate Tenant binding: keep the resource and version unchanged.
		source.clone()
	} else {
		store
			.add_version(
				destination_tenant,
				&resource,
				&"destination-provider-key".into(),
			)
			.await
			.unwrap()
	};
	let mut tx = f.database.connection.begin().await.unwrap();
	let rows = Version::objects()
		.order_by(&["created_at"])
		.all_with_executor(tx.as_mut())
		.await
		.unwrap();
	let mut copied = rows[if destination_tenant == "beta" { 0 } else { 1 }].clone();
	copied.tenant = destination_tenant.into();
	copied.nonce = rows[0].nonce.clone();
	copied.ciphertext = rows[0].ciphertext.clone();
	Version::objects()
		.save_with_executor(tx.as_mut(), &copied)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	assert_eq!(
		store
			.read(destination_tenant, &resource, &dest)
			.await
			.unwrap_err()
			.to_string(),
		"Provider Credential Store cannot read the pinned version"
	);
	if destination_tenant == "alpha" {
		assert_eq!(
			store
				.read("alpha", &a_resource, &source)
				.await
				.unwrap()
				.expose_secret(),
			"source-provider-key"
		);
	}
}

#[rstest]
#[tokio::test]
async fn postgres_store_key_registry_handles_concurrent_boot_rotation_wrong_key_and_tampering(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_application::provider_access::KeyMaterialReader;
	use aidash_server::apps::identity::{
		models::credential_store::RegisteredKey, repositories::credential_store::PostgresStore,
	};
	use reinhardt::db::orm::Model;
	let f = endpoint.await;
	let pool = f.runtime.store.control_pool.clone();
	let old = "33".repeat(32);
	let new = "44".repeat(32);
	let (a, b) = tokio::join!(
		PostgresStore::new(pool.clone(), old.clone().into(), vec![]),
		PostgresStore::new(pool.clone(), old.clone().into(), vec![])
	);
	let a = a.unwrap();
	b.unwrap();
	let id = Uuid::now_v7();
	a.create("alpha", id).await.unwrap();
	let resource = a.resource(id);
	let first = a
		.add_version("alpha", &resource, &"first-provider-key".into())
		.await
		.unwrap();
	assert!(
		PostgresStore::new(pool.clone(), "55".repeat(32).into(), vec![])
			.await
			.err()
			.unwrap()
			.to_string()
			.contains("does not match")
	);
	let mut tx = f.database.connection.begin().await.unwrap();
	assert_eq!(
		RegisteredKey::objects()
			.all()
			.all_with_executor(tx.as_mut())
			.await
			.unwrap()
			.len(),
		1
	);
	tx.commit().await.unwrap();
	let rotated = PostgresStore::new(pool.clone(), new.clone().into(), vec![old.clone().into()])
		.await
		.unwrap();
	assert_eq!(
		rotated
			.read("alpha", &resource, &first)
			.await
			.unwrap()
			.expose_secret(),
		"first-provider-key"
	);
	let second = rotated
		.add_version("alpha", &resource, &"second-provider-key".into())
		.await
		.unwrap();
	let retired_removed = PostgresStore::new(pool.clone(), new.clone().into(), vec![])
		.await
		.unwrap();
	assert!(
		retired_removed
			.read("alpha", &resource, &first)
			.await
			.is_err()
	);
	assert!(
		String::from_utf8(LOG.get().unwrap().lock().unwrap().clone())
			.unwrap()
			.contains("versions use an unconfigured Master Key")
	);
	retired_removed.disable(&first).await.unwrap();
	retired_removed.destroy(&first).await.unwrap();
	assert_eq!(
		retired_removed
			.read("alpha", &resource, &second)
			.await
			.unwrap()
			.expose_secret(),
		"second-provider-key"
	);
	retired_removed.delete(&resource).await.unwrap();
	let mut tx = f.database.connection.begin().await.unwrap();
	let rows = RegisteredKey::objects()
		.all()
		.all_with_executor(tx.as_mut())
		.await
		.unwrap();
	let mut row = rows[0].clone();
	row.check_ciphertext[0] ^= 1;
	RegisteredKey::objects()
		.save_with_executor(tx.as_mut(), &row)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	assert!(
		PostgresStore::new(pool, new.into(), vec![old.into()])
			.await
			.err()
			.unwrap()
			.to_string()
			.contains("check failed")
	);
}

#[rstest]
#[tokio::test]
async fn postgres_store_allocates_monotonic_versions_even_after_destroy(
	#[future] endpoint: EndpointFixture,
) {
	let f = endpoint.await;
	let store = postgres_store(&f, &"66".repeat(32), vec![]).await;
	let id = Uuid::now_v7();
	store.create("alpha", id).await.unwrap();
	let resource = store.resource(id);
	let first = SecretString::from("first-key");
	let other = SecretString::from("other-key");
	let (a, b) = tokio::join!(
		store.add_version("alpha", &resource, &first),
		store.add_version("alpha", &resource, &other)
	);
	let mut values = vec![a.unwrap(), b.unwrap()];
	values.sort();
	assert_eq!(
		values,
		vec![
			format!("{resource}/versions/1"),
			format!("{resource}/versions/2")
		]
	);
	store.destroy(&values[1]).await.unwrap();
	assert_eq!(
		store.versions(&resource).await.unwrap(),
		vec![values[0].clone()]
	);
	use aidash_application::provider_access::KeyMaterialReader;
	assert!(store.read("alpha", &resource, &values[1]).await.is_err());
	store.destroy(&values[1]).await.unwrap();
	assert_eq!(
		store
			.add_version("alpha", &resource, &"third-key".into())
			.await
			.unwrap(),
		format!("{resource}/versions/3")
	);
}

#[test]
fn store_settings_validate_shape_without_loading_keys() {
	use aidash_server::apps::identity::serializers::provider_credentials::Settings;
	use reinhardt::conf::settings::{fragment::SettingsValidation, profile::Profile};
	let current = json!({"file":"/definitely-missing/aidash157-master-key"});
	let fingerprint = json!({"env":"AIDASH_PROVIDER_FINGERPRINT_KEY"});
	let valid = json!({"fingerprint_key":fingerprint,"store":{"kind":"postgres","master_key":current,"retired_master_keys":[{"env":"AIDASH_PROVIDER_STORE_MASTER_KEY_OLD"}]}});
	serde_json::from_value::<Settings>(valid.clone())
		.unwrap()
		.validate(&Profile::parse("local"))
		.unwrap();
	for source in [
		json!({}),
		json!({"env":"AIDASH_SECRET_MASTER"}),
		json!({"file":"/tmp/key","env":"AIDASH_PROVIDER_KEY"}),
	] {
		let mut value = valid.clone();
		value["store"]["master_key"] = source.clone();
		assert!(
			serde_json::from_value::<Settings>(value)
				.unwrap()
				.validate(&Profile::parse("local"))
				.is_err()
		);
		let cloud = json!({"fingerprint_key":source,"store":{"kind":"secret_manager","byok_project_id":"byok-project","environment_id":"local"}});
		assert!(
			serde_json::from_value::<Settings>(cloud)
				.unwrap()
				.validate(&Profile::parse("local"))
				.is_err()
		);
	}
	let mut no_fingerprint = valid.clone();
	no_fingerprint
		.as_object_mut()
		.unwrap()
		.remove("fingerprint_key");
	assert!(
		serde_json::from_value::<Settings>(no_fingerprint)
			.unwrap()
			.validate(&Profile::parse("local"))
			.is_err()
	);
	let mut cloud_master = valid;
	cloud_master["store"] = json!({"kind":"secret_manager","byok_project_id":"byok-project","environment_id":"local","master_key":current});
	assert!(serde_json::from_value::<Settings>(cloud_master).is_err());
}

#[rstest]
#[tokio::test]
async fn bootstrap_loads_trimmed_keys_and_fails_closed_without_valid_key_sources(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_server::apps::identity::serializers::provider_credentials::Settings;
	let mut f = endpoint.await;
	let dir = tempfile::tempdir().unwrap();
	let master = dir.path().join("master");
	let fingerprint = dir.path().join("fingerprint");
	std::fs::write(&fingerprint, "   fingerprint-root-at-least-32-bytes-157\n").unwrap();
	let shape = json!({"fingerprint_key":{"file":fingerprint},"store":{"kind":"postgres","master_key":{"file":master}}});
	let settings: Settings = serde_json::from_value(shape.clone()).unwrap();
	assert!(
		aidash_server::bootstrap::configure_provider_credentials(&mut f.runtime.store, &settings)
			.await
			.unwrap_err()
			.to_string()
			.contains("Master Key file is missing or unreadable")
	);
	for malformed in ["", "short", &"gg".repeat(32)] {
		std::fs::write(&master, malformed).unwrap();
		assert!(
			aidash_server::bootstrap::configure_provider_credentials(
				&mut f.runtime.store,
				&settings
			)
			.await
			.unwrap_err()
			.to_string()
			.contains("64 hex characters")
		);
	}
	std::fs::write(&master, format!("  {}\n", "77".repeat(32))).unwrap();
	aidash_server::bootstrap::configure_provider_credentials(&mut f.runtime.store, &settings)
		.await
		.unwrap();
	assert!(f.runtime.store.provider_credentials.is_some());
	assert!(f.runtime.store.provider_key_material_reader.is_some());
	assert_eq!(
		f.runtime
			.store
			.provider_credentials
			.as_ref()
			.unwrap()
			.fingerprint_key
			.expose_secret(),
		"fingerprint-root-at-least-32-bytes-157"
	);
	std::fs::write(&master, "88".repeat(32)).unwrap();
	assert!(
		aidash_server::bootstrap::configure_provider_credentials(&mut f.runtime.store, &settings)
			.await
			.unwrap_err()
			.to_string()
			.contains("does not match")
	);
	let mut missing_env = shape;
	missing_env["store"]["master_key"] =
		json!({"env": format!("AIDASH_PROVIDER_STORE_MISSING_{}", Uuid::new_v4().simple())});
	let settings: Settings = serde_json::from_value(missing_env).unwrap();
	assert!(
		aidash_server::bootstrap::configure_provider_credentials(&mut f.runtime.store, &settings)
			.await
			.unwrap_err()
			.to_string()
			.contains("Master Key environment variable is missing")
	);

	let cloud: Settings = serde_json::from_value(json!({"fingerprint_key":{"file":fingerprint},"store":{"kind":"secret_manager","byok_project_id":"fixture-byok","environment_id":"local"}})).unwrap();
	aidash_server::bootstrap::configure_provider_credentials(&mut f.runtime.store, &cloud)
		.await
		.unwrap();
	assert!(f.runtime.store.provider_credentials.is_some());
	assert!(f.runtime.store.provider_key_material_reader.is_none());
}

#[rstest]
#[tokio::test]
async fn manage_migrate_does_not_load_configured_store_keys(#[future] endpoint: EndpointFixture) {
	let f = endpoint.await;
	let dir = tempfile::tempdir().unwrap();
	std::fs::write(
		dir.path().join("base.toml"),
		r#"
[provider_credentials]
fingerprint_key = { env = "AIDASH_PROVIDER_MISSING_FINGERPRINT_157" }
[provider_credentials.store]
kind = "postgres"
master_key = { file = "/definitely-missing/aidash157-master-key" }
"#,
	)
	.unwrap();
	let output = tokio::time::timeout(
		std::time::Duration::from_secs(45),
		tokio::process::Command::new(env!("CARGO_BIN_EXE_manage"))
			.arg("migrate")
			.env("DATABASE_URL", &f.database.url)
			.env("REINHARDT_SETTINGS_DIR", dir.path())
			.env("REINHARDT_ENV", "local")
			.env_remove("AIDASH_PROVIDER_MISSING_FINGERPRINT_157")
			.stdin(std::process::Stdio::null())
			.kill_on_drop(true)
			.output(),
	)
	.await
	.unwrap()
	.unwrap();
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
}

fn recovery_settings(f: &EndpointFixture, dir: &std::path::Path, current: &str) {
	std::fs::write(dir.join("master"), format!("  {current}\n")).unwrap();
	std::fs::write(
		dir.join("fingerprint"),
		" fingerprint-root-at-least-32-bytes-157\n",
	)
	.unwrap();
	std::fs::write(
		dir.join("base.toml"),
		format!(
			r#"[core]
secret_key = "recovery-fixture-secret-key-not-for-production-at-least-50-bytes-157"
[node]
node_id = {:?}
endpoint = "http://127.0.0.1:8080"
api_token = "recovery-fixture-token-at-least-16"
[provider_credentials]
fingerprint_key = {{ file = {:?} }}
[provider_credentials.store]
kind = "postgres"
master_key = {{ file = {:?} }}
"#,
			f.runtime.store.node_id,
			dir.join("fingerprint"),
			dir.join("master")
		),
	)
	.unwrap();
}

async fn recovery_command(
	f: &EndpointFixture,
	dir: &std::path::Path,
	execute: bool,
) -> std::process::Output {
	let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_manage"));
	command.arg("provider-credential-store-recovery");
	if execute {
		command.arg("--execute");
	}
	tokio::time::timeout(
		std::time::Duration::from_secs(45),
		command
			.env("DATABASE_URL", &f.database.url)
			.env("REINHARDT_SETTINGS_DIR", dir)
			.env("REINHARDT_ENV", "local")
			.stdin(std::process::Stdio::null())
			.kill_on_drop(true)
			.output(),
	)
	.await
	.unwrap()
	.unwrap()
}

async fn recovery_snapshot(f: &EndpointFixture) -> Value {
	let mut tx = f.database.connection.begin().await.unwrap();
	let mut snapshot = serde_json::Map::new();
	for name in [
		"credential_store_resources",
		"credential_store_versions",
		"credential_store_keys",
		"provider_credentials",
		"provider_credential_bindings",
		"events",
	] {
		let (sql, values) = Query::select()
			.expr_as(
				Expr::col(Alias::new("record")).cast_as(Alias::new("text")),
				Alias::new("document"),
			)
			.from_as(Alias::new(name), Alias::new("record"))
			.build(PostgresQueryBuilder);
		let mut rows: Vec<String> = tx
			.fetch_all(&sql, convert_values(values))
			.await
			.unwrap()
			.into_iter()
			.map(|r| r.get("document").unwrap())
			.collect();
		rows.sort();
		snapshot.insert(name.into(), json!(rows));
	}
	tx.commit().await.unwrap();
	Value::Object(snapshot)
}

fn real_service(
	f: &EndpointFixture,
	store: Arc<aidash_server::apps::identity::repositories::credential_store::PostgresStore>,
) -> Service {
	Service {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
			node: f.runtime.store.node_id.clone(),
		}),
		store,
		validator: Arc::new(Validator),
		fingerprint_key: "fingerprint-root-at-least-32-bytes-157".into(),
		max_per_tenant: 20,
	}
}

#[rstest]
#[case::total(false)]
#[case::partial(true)]
#[tokio::test]
async fn recovery_dry_run_and_execute_cover_total_partial_and_mixed_key_loss(
	#[future] endpoint: EndpointFixture,
	#[case] partial: bool,
) {
	use aidash_domain::provider_credentials::State;
	use aidash_server::apps::identity::models::credential_store::{RegisteredKey, Version};
	use reinhardt::db::orm::Model;
	let f = endpoint.await;
	let lost = "91".repeat(32);
	let current = "92".repeat(32);
	let old_store = postgres_store(&f, &lost, vec![]).await;
	let old_service = real_service(&f, old_store.clone());
	let alpha = old_service
		.create(
			"alpha",
			Uuid::now_v7(),
			Provider::Openrouter,
			"lost-alpha-material-157".into(),
			"actor",
		)
		.await
		.unwrap()
		.provider_credential;
	let beta = old_service
		.create(
			"beta",
			Uuid::now_v7(),
			Provider::Openrouter,
			"lost-beta-material-157".into(),
			"actor",
		)
		.await
		.unwrap()
		.provider_credential;
	let resource = old_store.resource(alpha.id);
	let mut unaffected = None;
	let mut known_before = Vec::<Version>::new();
	let mut known_keys_before = Vec::<RegisteredKey>::new();
	if partial {
		let known_store = postgres_store(&f, &current, vec![lost.clone().into()]).await;
		let known_service = real_service(&f, known_store.clone());
		known_service
			.rotate(
				"alpha",
				alpha.id,
				2,
				"known-alpha-material-157".into(),
				"actor",
			)
			.await
			.unwrap();
		old_service
			.rotate(
				"alpha",
				alpha.id,
				3,
				"lost-alpha-rotated-157".into(),
				"actor",
			)
			.await
			.unwrap();
		// A configured-key orphan must be disabled by normal Revocation but retained intact.
		known_store
			.add_version("alpha", &resource, &"known-orphan-material-157".into())
			.await
			.unwrap();
		let gamma = known_service
			.create(
				"gamma",
				Uuid::now_v7(),
				Provider::Openrouter,
				"known-gamma-material-157".into(),
				"actor",
			)
			.await
			.unwrap()
			.provider_credential;
		let mut scope = known_service.repository.begin("gamma").await.unwrap();
		unaffected = Some(serde_json::to_value(scope.get(gamma.id).await.unwrap()).unwrap());
		scope.commit().await.unwrap();
		let mut tx = f.database.connection.begin().await.unwrap();
		known_before = Version::objects()
			.all()
			.all_with_executor(tx.as_mut())
			.await
			.unwrap()
			.into_iter()
			.filter(|row| {
				row.resource == resource && (row.version == 2 || row.version == 4)
					|| row.tenant == "gamma"
			})
			.collect();
		let known_id = &known_before[0].key_id;
		known_keys_before = RegisteredKey::objects()
			.filter(RegisteredKey::field_key_id().eq(known_id))
			.all_with_executor(tx.as_mut())
			.await
			.unwrap();
		tx.commit().await.unwrap();
	} else {
		let error =
			aidash_server::apps::identity::repositories::credential_store::PostgresStore::new(
				f.runtime.store.control_pool.clone(),
				current.clone().into(),
				vec![],
			)
			.await
			.err()
			.unwrap();
		assert!(
			error
				.to_string()
				.contains("manage provider-credential-store-recovery")
		);
	}
	let dir = tempfile::tempdir().unwrap();
	recovery_settings(&f, dir.path(), &current);
	let before = recovery_snapshot(&f).await;
	let counts = json!({"affected_tenants":2,"affected_provider_credentials":2,"version_rows":if partial {3} else {2},"key_registry_rows":1});
	let mut tx = f.database.connection.begin().await.unwrap();
	let mut private_values = Vec::new();
	for row in Version::objects()
		.all()
		.all_with_executor(tx.as_mut())
		.await
		.unwrap()
	{
		private_values.push(row.key_id);
		for bytes in [row.nonce, row.ciphertext] {
			private_values.push(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
			private_values.push(serde_json::to_string(&bytes).unwrap());
		}
	}
	for row in RegisteredKey::objects()
		.all()
		.all_with_executor(tx.as_mut())
		.await
		.unwrap()
	{
		private_values.push(row.key_id);
		for bytes in [row.check_nonce, row.check_ciphertext] {
			private_values.push(bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
			private_values.push(serde_json::to_string(&bytes).unwrap());
		}
	}
	tx.commit().await.unwrap();
	for execute in [false, true] {
		let output = recovery_command(&f, dir.path(), execute).await;
		assert!(
			output.status.success(),
			"{}",
			String::from_utf8_lossy(&output.stderr)
		);
		assert_eq!(
			serde_json::from_slice::<Value>(&output.stdout).unwrap(),
			counts
		);
		let printed = format!(
			"{}{}",
			String::from_utf8_lossy(&output.stdout),
			String::from_utf8_lossy(&output.stderr)
		);
		for forbidden in [
			&lost,
			&current,
			"lost-alpha-material-157",
			"lost-beta-material-157",
		] {
			assert!(!printed.contains(forbidden));
		}
		for forbidden in &private_values {
			assert!(!printed.contains(forbidden));
		}
		for table in ["credential_store_versions", "credential_store_keys"] {
			for row in before[table].as_array().unwrap() {
				// Full persisted rows (including ciphertext, nonces and identifiers) never print.
				assert!(!printed.contains(row.as_str().unwrap()));
			}
		}
		if !execute {
			assert_eq!(recovery_snapshot(&f).await, before);
		}
	}
	for (tenant, id) in [("alpha", alpha.id), ("beta", beta.id)] {
		let mut scope = old_service.repository.begin(tenant).await.unwrap();
		let row = scope.get(id).await.unwrap();
		assert_eq!(row.state, State::Revoked);
		assert_eq!(
			row.revision,
			if tenant == "alpha" && partial { 5 } else { 3 }
		);
		scope.commit().await.unwrap();
	}
	let mut tx = f.database.connection.begin().await.unwrap();
	let remaining = Version::objects()
		.all()
		.all_with_executor(tx.as_mut())
		.await
		.unwrap();
	assert_eq!(remaining.len(), if partial { 3 } else { 0 });
	for before in &known_before {
		let after = remaining
			.iter()
			.find(|r| r.resource == before.resource && r.version == before.version)
			.unwrap();
		if before.tenant == "gamma" {
			assert_eq!(
				serde_json::to_value(after).unwrap(),
				serde_json::to_value(before).unwrap()
			);
		} else {
			assert_eq!(after.state, "disabled");
			let mut expected = serde_json::to_value(before).unwrap();
			if before.state == "enabled" {
				assert!(after.disabled_at.is_some());
				expected["state"] = json!("disabled");
				expected["disabled_at"] = json!(after.disabled_at);
			}
			assert_eq!(serde_json::to_value(after).unwrap(), expected);
		}
	}
	let keys = RegisteredKey::objects()
		.all()
		.all_with_executor(tx.as_mut())
		.await
		.unwrap();
	assert_eq!(
		serde_json::to_value(&keys).unwrap(),
		serde_json::to_value(&known_keys_before).unwrap()
	);
	let (sql, values) = Query::select()
		.expr_as(
			Expr::col("data").cast_as(Alias::new("text")),
			Alias::new("data"),
		)
		.from(Alias::new("events"))
		.and_where(Expr::col("kind").eq(Expr::value("provider_credential.revoked")))
		.build(PostgresQueryBuilder);
	let events = tx.fetch_all(&sql, convert_values(values)).await.unwrap();
	assert_eq!(events.len(), 2);
	let mut tenants = std::collections::BTreeSet::new();
	for event in events {
		let data: Value = serde_json::from_str(&event.get::<String>("data").unwrap()).unwrap();
		assert_eq!(data["actor"], "provider-credential-store-recovery");
		assert_eq!(data["state"], "revoked");
		tenants.insert(data["tenant"].as_str().unwrap().to_owned());
	}
	assert_eq!(
		tenants,
		std::collections::BTreeSet::from(["alpha".into(), "beta".into()])
	);
	tx.commit().await.unwrap();
	assert_eq!(
		recovery_snapshot(&f).await["credential_store_resources"],
		before["credential_store_resources"]
	);
	if let Some(before) = unaffected {
		let mut scope = old_service.repository.begin("gamma").await.unwrap();
		assert_eq!(
			serde_json::to_value(
				scope
					.get(serde_json::from_value(before["id"].clone()).unwrap())
					.await
					.unwrap()
			)
			.unwrap(),
			before
		);
		scope.commit().await.unwrap();
	}
	// Recovery never registers the replacement; ordinary startup now can.
	let recovered = postgres_store(&f, &current, vec![]).await;
	let fresh = real_service(&f, recovered.clone())
		.create(
			"reconnected",
			Uuid::now_v7(),
			Provider::Openrouter,
			"reconnected-provider-material-157".into(),
			"oauth-callback",
		)
		.await
		.unwrap()
		.provider_credential;
	use aidash_application::provider_access::KeyMaterialReader;
	assert_eq!(
		recovered
			.read(
				"reconnected",
				&recovered.resource(fresh.id),
				&format!("{}/versions/1", recovered.resource(fresh.id))
			)
			.await
			.unwrap()
			.expose_secret(),
		"reconnected-provider-material-157"
	);
	let output = recovery_command(&f, dir.path(), true).await;
	assert!(output.status.success());
	assert_eq!(
		serde_json::from_slice::<Value>(&output.stdout).unwrap(),
		json!({"affected_tenants":0,"affected_provider_credentials":0,"version_rows":0,"key_registry_rows":0})
	);
}

#[rstest]
#[case::current(false)]
#[case::retired(true)]
#[tokio::test]
async fn recovery_refuses_tampered_configured_key_checks_without_mutation(
	#[future] endpoint: EndpointFixture,
	#[case] retired: bool,
) {
	use aidash_server::apps::identity::models::credential_store::RegisteredKey;
	use reinhardt::db::orm::Model;
	let f = endpoint.await;
	let lost = "93".repeat(32);
	let current = "94".repeat(32);
	let old = postgres_store(&f, &lost, vec![]).await;
	real_service(&f, old)
		.create(
			"alpha",
			Uuid::now_v7(),
			Provider::Openrouter,
			"tamper-lost-material-157".into(),
			"actor",
		)
		.await
		.unwrap();
	let mut tx = f.database.connection.begin().await.unwrap();
	let old_id = RegisteredKey::objects()
		.all()
		.all_with_executor(tx.as_mut())
		.await
		.unwrap()[0]
		.key_id
		.clone();
	tx.commit().await.unwrap();
	postgres_store(&f, &current, vec![lost.clone().into()]).await;
	let mut tx = f.database.connection.begin().await.unwrap();
	for mut row in RegisteredKey::objects()
		.all()
		.all_with_executor(tx.as_mut())
		.await
		.unwrap()
	{
		if (row.key_id == old_id) == retired {
			row.check_ciphertext[0] ^= 1;
			RegisteredKey::objects()
				.save_with_executor(tx.as_mut(), &row)
				.await
				.unwrap();
		}
	}
	tx.commit().await.unwrap();
	let dir = tempfile::tempdir().unwrap();
	recovery_settings(&f, dir.path(), &current);
	if retired {
		std::fs::write(dir.path().join("retired"), lost).unwrap();
		let settings = std::fs::read_to_string(dir.path().join("base.toml")).unwrap();
		std::fs::write(
			dir.path().join("base.toml"),
			format!(
				"{settings}retired_master_keys = [{{ file = {:?} }}]\n",
				dir.path().join("retired")
			),
		)
		.unwrap();
	}
	let before = recovery_snapshot(&f).await;
	for execute in [false, true] {
		let output = recovery_command(&f, dir.path(), execute).await;
		assert!(!output.status.success());
		assert!(String::from_utf8_lossy(&output.stderr).contains("Master Key check failed"));
		assert_eq!(recovery_snapshot(&f).await, before);
	}
}

#[rstest]
#[tokio::test]
async fn recovery_requires_postgres_and_valid_key_sources(#[future] endpoint: EndpointFixture) {
	let f = endpoint.await;
	let dir = tempfile::tempdir().unwrap();
	let before = recovery_snapshot(&f).await;
	for shape in [
		"[provider_credentials]\n",
		"[provider_credentials]\nfingerprint_key = { env = \"AIDASH_PROVIDER_MISSING_157\" }\n[provider_credentials.store]\nkind = \"secret_manager\"\nbyok_project_id = \"byok-project\"\nenvironment_id = \"local\"\n",
	] {
		recovery_settings(&f, dir.path(), &"95".repeat(32));
		let text = std::fs::read_to_string(dir.path().join("base.toml")).unwrap();
		std::fs::write(
			dir.path().join("base.toml"),
			format!(
				"{}{}",
				text.split("[provider_credentials]").next().unwrap(),
				shape
			),
		)
		.unwrap();
		let output = recovery_command(&f, dir.path(), false).await;
		assert!(!output.status.success());
		assert!(String::from_utf8_lossy(&output.stderr).contains("requires a PostgreSQL Store"));
	}
	for key in [None, Some("malformed")] {
		recovery_settings(&f, dir.path(), &"95".repeat(32));
		if let Some(value) = key {
			std::fs::write(dir.path().join("master"), value).unwrap();
		} else {
			std::fs::remove_file(dir.path().join("master")).unwrap();
		}
		for execute in [false, true] {
			let output = recovery_command(&f, dir.path(), execute).await;
			assert!(!output.status.success());
			assert!(
				String::from_utf8_lossy(&output.stderr).contains(if key.is_some() {
					"64 hex characters"
				} else {
					"Master Key file is missing"
				})
			);
		}
	}
	assert_eq!(recovery_snapshot(&f).await, before);
}
