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
use secrecy::SecretString;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, OnceLock};
use uuid::Uuid;

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
	async fn create(&self, id: Uuid) -> Result<()> {
		self.0.lock().unwrap().insert(self.resource(id), vec![]);
		Ok(())
	}
	async fn add_version(&self, resource: &str, _: &SecretString) -> Result<String> {
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
				SimpleExpr::FunctionCall(
					"row_to_json".into_iden(),
					vec![Expr::col(Alias::new("record")).into()],
				),
				Alias::new("document"),
			)
			.from_as(Alias::new(&name), Alias::new("record"))
			.build(PostgresQueryBuilder);
		for row in tx.fetch_all(&sql, convert_values(values)).await.unwrap() {
			let reinhardt::db::backends::QueryValue::Json(Some(value)) = &row.data["document"]
			else {
				panic!("row_to_json must return JSON");
			};
			result.push_str(&value.to_string());
		}
	}
	result
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
	f.runtime.registry.register(model.clone()).await.unwrap();
	for seconds in [3601_u32, u32::MAX] {
		let mut invalid = model.clone();
		invalid.id = format!("byok-too-long-{seconds}");
		invalid.config["request_timeout_secs"] = json!(seconds);
		assert!(
			f.runtime
				.registry
				.register(invalid)
				.await
				.unwrap_err()
				.to_string()
				.contains("at most 3600")
		);
	}
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
		inference: None,
	};
	let mut maintenance = Context {
		tenant: "beta".into(),
		run: None,
		maintenance: None,
		// Supplied IDs cannot select another Tenant's record for maintenance.
		provider_credential_id: Some(a.id),
		inference: None,
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
	// Native admission feeds the worker-local issuer, retaining the Run pin and
	// selecting the current mapped-local binding for Run-less maintenance.
	struct Issuer(Arc<Mutex<Vec<(Context, Uuid, String)>>>);
	#[async_trait]
	impl aidash_application::provider_access::TokenIssuer for Issuer {
		async fn mint(
			&self,
			context: &Context,
			row: &aidash_domain::provider_credentials::ProviderCredential,
		) -> Result<aidash_application::provider_access::Access> {
			self.0
				.lock()
				.unwrap()
				.push((context.clone(), row.id, row.require_active()?.into()));
			Ok(aidash_application::provider_access::Access {
				endpoint: "https://broker.test/api/v1".into(),
				bearer: "fixture-token".into(),
			})
		}
	}
	let mints = Arc::new(Mutex::new(Vec::new()));
	f.runtime.store.capability_issuer = Some(Arc::new(Issuer(mints.clone())));
	let enabled = AdmittedAccess {
		store: f.runtime.store.clone(),
	};
	let mut scoped = context.clone();
	scoped.inference = Some(aidash_application::provider_access::Inference {
		model: "test/model".into(),
		operations: vec![aidash_application::provider_access::Operation::Chat],
		max_output_tokens: 1024,
	});
	assert_eq!(
		enabled
			.resolve(&scoped, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap()
			.endpoint,
		"https://broker.test/api/v1"
	);
	let mut scoped_maintenance = maintenance.clone();
	scoped_maintenance.inference = scoped.inference.clone();
	enabled
		.resolve(
			&scoped_maintenance,
			Provider::Openrouter.base_url(),
			&source,
		)
		.await
		.unwrap();
	let mut metadata = service.repository.begin("beta").await.unwrap();
	let current = metadata.get(b.id).await.unwrap();
	let current_pin = current.require_active().unwrap().to_owned();
	metadata.commit().await.unwrap();
	{
		let captured = mints.lock().unwrap();
		assert_eq!(captured.len(), 2);
		assert_eq!(captured[0].0.tenant, "beta");
		assert_eq!(captured[0].0.run, Some(run));
		assert_eq!(
			captured[0].0.inference.as_ref().unwrap().model,
			"test/model"
		);
		assert_eq!(captured[0].1, b.id);
		assert_eq!(captured[0].2, current_pin);
		assert_eq!(captured[1].0.run, None);
		assert_eq!(captured[1].1, replacement.id);
	}
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
