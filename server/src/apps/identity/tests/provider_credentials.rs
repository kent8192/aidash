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
struct FakeStore {
	secrets: Mutex<std::collections::BTreeMap<String, Vec<String>>>,
	fail_create: bool,
}
#[async_trait]
impl Store for FakeStore {
	fn resource(&self, id: Uuid) -> String {
		format!("fake/{id}")
	}
	async fn create(&self, id: Uuid) -> Result<()> {
		if self.fail_create {
			return Err(aidash_application::Error::External(
				"Provider Credential Store unavailable".into(),
			));
		}
		self.secrets
			.lock()
			.unwrap()
			.insert(self.resource(id), vec![]);
		Ok(())
	}
	async fn add_version(&self, resource: &str, _: &SecretString) -> Result<String> {
		let mut values = self.secrets.lock().unwrap();
		let versions = values.get_mut(resource).unwrap();
		let name = format!("{resource}/versions/{}", versions.len() + 1);
		versions.push(name.clone());
		Ok(name)
	}
	async fn versions(&self, resource: &str) -> Result<Vec<String>> {
		Ok(self
			.secrets
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
		self.secrets.lock().unwrap().remove(resource);
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

async fn authorize_tenant(f: &EndpointFixture, tenant: &str) {
	assert_json(f.operator.post(&format!("/api/authorization/{tenant}"), &json!({
		"expected_revision":0,"bundle":{"tenant":tenant,"subjects":{"actor":{"kind":"user"},aidash_domain::qualified_agent(&f.runtime.config.node_id,"environment-agent","1.0.0"):{"kind":"agent"}},
		"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]}
	}), "json").await.unwrap(),200);
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
	let cleanup = own_events
		.as_array()
		.unwrap()
		.iter()
		.find(|event| {
			event["kind"] == "provider_credential.cleanup_completed" && event["data"]["id"] == id
		})
		.expect("the Tenant's authorized cleanup event must reach polling");
	assert_eq!(cleanup["data"]["state"], "deleted");
	let own_state = assert_json(alpha_subject.get("/api/state").await.unwrap(), 200);
	assert!(
		own_state["events"]
			.as_array()
			.unwrap()
			.iter()
			.any(|event| event["id"] == cleanup["id"])
	);
	// Stream the same stored event through the production Subject SSE route.
	use futures_util::StreamExt as _;
	let response = reqwest::Client::new()
		.get(format!(
			"{}/api/events/stream?after={}",
			f.server.url,
			cleanup["sequence"].as_i64().unwrap() - 1
		))
		.bearer_auth(alpha_token["token"].as_str().unwrap())
		.send()
		.await
		.unwrap();
	assert_eq!(response.status().as_u16(), 200);
	let mut stream = response.bytes_stream();
	let frame = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	let frame = String::from_utf8_lossy(&frame);
	assert!(frame.contains("provider_credential.cleanup_completed"));
	assert!(frame.contains(id));
	assert!(!frame.contains(canary));
	drop(stream);
	assert!(!own_events.to_string().contains(canary));
	let other_events = assert_json(subject.get("/api/events?after=0").await.unwrap(), 200);
	assert!(!other_events.to_string().contains(id));
	let other_state = assert_json(subject.get("/api/state").await.unwrap(), 200);
	assert!(!other_state["events"].to_string().contains(id));
	// Matching the Tenant is insufficient when Provider Credential read is denied.
	let denied = json!({"tenant":"alpha","subjects":{"alice":{"kind":"user"}},"policies":[]});
	assert_json(
		f.operator
			.post(
				"/api/authorization/alpha",
				&json!({"expected_revision":1,"bundle":denied}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let denied_events = assert_json(alpha_subject.get("/api/events?after=0").await.unwrap(), 200);
	assert!(
		!denied_events
			.to_string()
			.contains(cleanup["id"].as_str().unwrap())
	);
	let denied_state = assert_json(alpha_subject.get("/api/state").await.unwrap(), 200);
	assert!(
		!denied_state["events"]
			.to_string()
			.contains(cleanup["id"].as_str().unwrap())
	);
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
async fn workspace_embedding_credential_is_pinned_with_environment_model_admission(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_application::provider_access::{Context, ProviderAccess, Source};
	use aidash_server::apps::identity::models::{
		AuthorizationWorkspace, provider_credentials::RunProviderCredential as Pin,
	};
	use aidash_server::apps::knowledge::models::SemanticIndexe;
	use reinhardt::db::orm::{Json, Model};
	let mut f = endpoint.await;
	authorize_tenant(&f, "beta").await;
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
	f.runtime.store.provider_credentials = Some(service.clone());
	f.runtime.registry = f.runtime.registry.clone().with_provider_credentials(true);
	for (kind, id, config) in [
		(
			"model",
			"environment-agent-fixture-model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:1/v1","credential_env":null,"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"agent",
			"environment-agent",
			json!({"schema_version":1,"model":{"id":"environment-agent-fixture-model","version":"1.0.0"},"instructions":"Embedding admission fixture","bindings":[],"remove_default":["file_search","file_read"]}),
		),
	] {
		f.runtime.registry.register(serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Embedding admission fixture"},"config":config})).unwrap()).await.unwrap();
	}

	f.context.set_singleton(f.runtime.clone());
	for id in ["environment-agent-fixture-model", "environment-agent"] {
		assert_json(
			f.operator
				.post(
					"/api/authorization/beta/catalog",
					&json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
	}
	let token = assert_json(
		f.operator
			.post(
				"/api/authorization/beta/credentials",
				&json!({"subject":"actor"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let actor = reinhardt::test::fixtures::api_client_from_url(&f.server.url);
	actor
		.set_header(
			"Authorization",
			&format!("Bearer {}", token["token"].as_str().unwrap()),
		)
		.await
		.unwrap();
	let workspace = f
		.runtime
		.store
		.create_workspace("BYOK retrieval", "Test admission")
		.await
		.unwrap();
	let mut tx = f.database.connection.begin().await.unwrap();
	let authority = AuthorizationWorkspace::build()
		.workspace_id(workspace.id)
		.tenant("beta")
		.owner_subject("actor")
		.finish();
	AuthorizationWorkspace::objects()
		.insert_with_executor(tx.as_mut(), &authority)
		.await
		.unwrap();
	let index = SemanticIndexe::build()
		.workspace_id(workspace.id).tenant("beta").revision(1)
		.spec(Json(json!({
			"embedding":{"provider":"openrouter","endpoint":"https://openrouter.ai/api/v1","credential_env":null,"provider_credential":"openrouter","model":"fixture","model_version":"1","dimensions":3},
			"vector":{"provider":"postgres","endpoint":"local","credential_env":null},
			"enabled":true,"auto_context":true,"max_sources":64,"max_results":10,"max_result_tokens":4096,"max_input_bytes":8192
		})))
		.collection("workspace-embedding-fixture").finish();
	SemanticIndexe::objects()
		.insert_with_executor(tx.as_mut(), &index)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let task = f
		.runtime
		.store
		.create_task(
			workspace.id,
			&aidash_domain::NewTask {
				title: "Embedding admission".into(),
				description: "Env model, Tenant embedding".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"actor",
			None,
		)
		.await
		.unwrap();
	let claim_path = format!("/api/tasks/{}/claim", task.id);
	let claim =
		json!({"revision":task.revision,"agent":{"id":"environment-agent","version":"1.0.0"}});
	let rejected = assert_json(actor.post(&claim_path, &claim, "json").await.unwrap(), 400);
	assert!(
		rejected.to_string().contains("binding is missing"),
		"{rejected}"
	);
	assert_eq!(
		f.runtime.store.task(task.id).await.unwrap().status,
		aidash_domain::TaskStatus::Open
	);
	// The failed admission must roll back its Run as well as every credential pin.
	assert!(f.runtime.store.runs().await.unwrap().is_empty());
	let credential = service
		.create(
			"beta",
			Uuid::now_v7(),
			Provider::Openrouter,
			"beta-embedding-provider-key-5678".into(),
			"actor",
		)
		.await
		.unwrap()
		.provider_credential;
	service
		.bind("beta", Provider::Openrouter, credential.id, 0, "actor")
		.await
		.unwrap();
	assert_json(actor.post(&claim_path, &claim, "json").await.unwrap(), 200);
	let run = f.runtime.store.runs().await.unwrap().remove(0);
	let mut tx = f.database.connection.begin().await.unwrap();
	let pins = Pin::objects()
		.filter(Pin::field_run_id().eq(run.id))
		.all_with_executor(tx.as_mut())
		.await
		.unwrap();
	assert_eq!(pins.len(), 1);
	assert_eq!(pins[0].tenant, "beta");
	assert_eq!(pins[0].provider, "openrouter");
	assert_eq!(pins[0].provider_credential_id, credential.id);
	tx.commit().await.unwrap();
	let access =
		aidash_server::apps::identity::repositories::provider_credentials::AdmittedAccess {
			store: f.runtime.store.clone(),
		};
	let source = Source::Tenant {
		provider: "openrouter".into(),
	};
	let context = Context {
		tenant: "beta".into(),
		run: Some(run.id),
		maintenance: None,
		provider_credential_id: None,
	};
	// Resolution reaches the deliberately unbound broker, proving it found the
	// admitted embedding pin rather than failing at the Env-only Agent closure.
	assert!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("credential broker not configured")
	);
	service
		.bind("beta", Provider::Openrouter, None, 1, "actor")
		.await
		.unwrap();
	assert!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("credential broker not configured")
	);
	service
		.revoke("beta", credential.id, credential.revision, "actor")
		.await
		.unwrap();
	assert!(
		access
			.resolve(&context, Provider::Openrouter.base_url(), &source)
			.await
			.unwrap_err()
			.to_string()
			.contains("not active")
	);
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
	authorize_tenant(&f, "beta").await;
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
	let model: aidash_server::registry::Entry = serde_json::from_value(json!({"id":"tenant-model","version":"1.0.0","kind":"model","name":{"en":"Tenant model"},"description":{"en":"Tenant model"},"config":{"provider":"openrouter","model_id":"test/model","endpoint":"https://openrouter.ai/api/v1","credential_env":null,"provider_credential":"openrouter","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
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
	f.context.set_singleton(f.runtime.clone());
	let draft = assert_json(f.operator.post("/api/workbench/drafts", &json!({
		"tenant":"beta","owner":"actor","entry":{
			"id":"","version":"1.0.0","kind":"agent","name":{"en":"Tenant sandbox"},"description":{"en":"Tenant sandbox"},
			"config":{"schema_version":1,"model":{"id":"tenant-model","version":"1.0.0"},"instructions":"Test admission","bindings":[],"remove_default":["memory_mutate","memory_recall","memory_reflect"]}
		}
	}), "json").await.unwrap(),200);
	// Leave enough fixture headroom for the built-in catalog so this request
	// exercises credential admission rather than the independent input bound.
	assert_json(f.operator.put("/api/workbench/test-limits/beta", &json!({
		"tenant":"beta","max_input_bytes":1000000,"max_output_tokens":1024,"max_total_tokens":1000000,
		"max_steps":4,"max_duration_secs":30,"max_concurrent":1,"payload_days":1,"incident_evidence_days":1
	}), "json").await.unwrap(),200);
	let sandbox_path = format!(
		"/api/workbench/drafts/{}/tests",
		draft["id"].as_str().unwrap()
	);
	let rejected = assert_json(
		f.operator
			.post(
				&sandbox_path,
				&json!({"expected_revision":draft["revision"],"message":"Test the Tenant model"}),
				"json",
			)
			.await
			.unwrap(),
		400,
	);
	assert!(
		rejected
			.to_string()
			.contains("Workbench tests do not support Tenant Provider Credentials"),
		"{rejected}"
	);
	let sessions = assert_json(f.operator.get(&sandbox_path).await.unwrap(), 200);
	assert_eq!(
		sessions,
		json!([]),
		"rejection must precede sandbox session admission"
	);
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
	let semantic = definition(tx.as_mut(), "semantic_indexes_revision").await;
	apply_asset(
		tx.as_mut(),
		&f.database.connection,
		include_str!("../../../../migrations/knowledge/sql/backward/0027_provider_credentials.sql"),
	)
	.await;
	assert_eq!(
		definition(tx.as_mut(), "semantic_indexes_revision").await,
		semantic.replace("'provider_credential'::text, ", "")
	);
	apply_asset(
		tx.as_mut(),
		&f.database.connection,
		include_str!("../../../../migrations/knowledge/sql/forward/0027_provider_credentials.sql"),
	)
	.await;
	assert_eq!(
		definition(tx.as_mut(), "semantic_indexes_revision").await,
		semantic
	);
	tx.rollback().await.unwrap();
}

#[rstest]
#[tokio::test]
async fn native_cleanup_inventory_is_bounded_ordered_and_includes_revocations(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_application::provider_credentials::{RECONCILIATION_BATCH_SIZE, Repository};
	use aidash_domain::provider_credentials::{ProviderCredential, State};
	let f = endpoint.await;
	let repository = NativeRepository {
		pool: f.runtime.store.control_pool.clone(),
		node: f.runtime.store.node_id.clone(),
	};
	let mut expected = Vec::new();
	for tenant in ["alpha", "beta"] {
		let mut scope = repository.begin(tenant).await.unwrap();
		for ordinal in 0..18 {
			let id = Uuid::now_v7();
			let state = [
				State::Pending,
				State::Active,
				State::Revoked,
				State::Deleted,
			][ordinal % 4];
			let cleanup_pending = state == State::Deleted && ordinal % 8 == 3;
			if state != State::Deleted || cleanup_pending {
				expected.push(id);
			}
			scope
				.insert(&ProviderCredential {
					id,
					tenant: tenant.into(),
					provider: Provider::Openrouter,
					base_url: Provider::Openrouter.base_url().into(),
					secret_resource: format!("fake/{id}"),
					pinned_version: if state == State::Pending
						|| (state == State::Deleted && !cleanup_pending)
					{
						None
					} else {
						Some(format!("fake/{id}/versions/1"))
					},
					fingerprint: "0123456789abcdef".into(),
					last4: "test".into(),
					state,
					created_at: chrono::Utc::now(),
					rotated_at: None,
					revoked_at: if state == State::Revoked {
						Some(chrono::Utc::now())
					} else {
						None
					},
					revision: 1,
				})
				.await
				.unwrap();
		}
		scope.commit().await.unwrap();
	}
	expected.sort();
	let first = repository
		.reconciliation_candidates(None, usize::MAX)
		.await
		.unwrap();
	assert_eq!(first.len(), RECONCILIATION_BATCH_SIZE);
	assert!(first.iter().any(|row| row.state == State::Pending));
	assert!(first.iter().any(|row| row.state == State::Revoked));
	assert!(first.iter().any(|row| row.state == State::Deleted));
	let second = repository
		.reconciliation_candidates(first.last().map(|row| row.id), usize::MAX)
		.await
		.unwrap();
	let actual: Vec<_> = first.into_iter().chain(second).map(|row| row.id).collect();
	assert_eq!(actual, expected);
	assert_eq!(
		repository
			.reconciliation_candidates(expected.last().copied(), 25)
			.await
			.unwrap()
			.len(),
		0
	);
}

#[rstest]
#[tokio::test]
async fn subject_mutations_return_committed_results_when_decision_audit_fails(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_server::{
		apps::identity::{
			models::AuthorizationDecision, services::provider_credentials::Management,
		},
		authorization::Authorization,
	};
	use reinhardt::{db::orm::Model, query::TableConstraint};
	let mut f = endpoint.await;
	let bundle = json!({"tenant":"alpha","subjects":{"alice":{"kind":"user"}},"policies":[{"id":"provider-access","effect":"allow","subjects":{"ids":["alice"]},"actions":["provider_credential.create","provider_credential.read","provider_credential.rotate","provider_credential.revoke","provider_credential.delete","provider_credential_binding.read","provider_credential_binding.update"],"resources":{"kinds":["provider_credential","provider_credential_binding"]}}]});
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
	let issued = assert_json(
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
	let token = issued["token"].as_str().unwrap();
	let actor = Authorization {
		pool: f.runtime.store.pool.clone(),
	}
	.authenticate(token)
	.await
	.unwrap();
	let subject = reinhardt::test::fixtures::api_client_from_url(&f.server.url);
	subject
		.set_header("Authorization", &format!("Bearer {token}"))
		.await
		.unwrap();
	let store = Arc::new(FakeStore::default());
	let service = Arc::new(Service {
		repository: Arc::new(NativeRepository {
			pool: f.runtime.store.control_pool.clone(),
			node: f.runtime.store.node_id.clone(),
		}),
		store: store.clone(),
		validator: Arc::new(Validator),
		fingerprint_key: "test-fingerprint-key-that-is-at-least-32-bytes".into(),
		max_per_tenant: 2,
	});
	f.runtime.store.provider_credentials = Some(service.clone());
	f.context.set_singleton(f.runtime.clone());
	let management = Management {
		runtime: f.runtime.clone(),
	};
	// A typed CHECK fails only the later authorization-decision append. The
	// lifecycle and binding transactions, including their own events, commit.
	let statement = Query::alter_table()
		.table(Alias::new("authorization_decisions"))
		.add_constraint(TableConstraint::Check {
			name: Some(Alias::new("fixture_provider_audit_failure").into_iden()),
			expr: Expr::col("action").is_not_in([
				"provider_credential.create",
				"provider_credential.rotate",
				"provider_credential.revoke",
				"provider_credential.delete",
				"provider_credential_binding.update",
			]),
		})
		.to_string(PostgresQueryBuilder);
	let mut tx = f.database.connection.begin().await.unwrap();
	tx.execute(&statement, vec![]).await.unwrap();
	tx.commit().await.unwrap();
	let created = management
		.create(
			actor.clone(),
			"alpha".into(),
			Provider::Openrouter,
			"valid-provider-key-1234".into(),
		)
		.await
		.unwrap()
		.provider_credential;
	let id = created.id;
	assert_eq!(created.revision, 2);
	let rotated = management
		.rotate(
			actor,
			"alpha".into(),
			id,
			created.revision,
			"rotated-provider-key-5678".into(),
		)
		.await
		.unwrap()
		.provider_credential;
	assert_eq!(rotated.revision, 3);
	let binding = assert_json(
		subject
			.put(
				"/api/tenants/alpha/provider-credential-bindings/openrouter",
				&json!({"expected_revision":0,"provider_credential_id":id}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(binding["revision"], 1);
	let revoke_path = format!("/api/tenants/alpha/provider-credentials/{id}/revoke");
	let revoked = assert_json(
		subject
			.post(
				&revoke_path,
				&json!({"expected_revision":rotated.revision}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(revoked["state"], "revoked");
	assert_eq!(revoked["revision"], 4);
	assert_eq!(
		subject
			.post(
				&revoke_path,
				&json!({"expected_revision":rotated.revision}),
				"json"
			)
			.await
			.unwrap()
			.status_code(),
		409
	);
	let delete_path = format!("/api/tenants/alpha/provider-credentials/{id}");
	let client = reqwest::Client::new();
	let response = client
		.delete(format!("{}{delete_path}", f.server.url))
		.bearer_auth(token)
		.json(&json!({"expected_revision":4}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status().as_u16(), 409);
	assert_eq!(response.headers()["cache-control"], "no-store");
	let unbound = assert_json(
		subject
			.put(
				"/api/tenants/alpha/provider-credential-bindings/openrouter",
				&json!({"expected_revision":1,"provider_credential_id":null}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(unbound["revision"], 2);
	let response = client
		.delete(format!("{}{delete_path}", f.server.url))
		.bearer_auth(token)
		.json(&json!({"expected_revision":4}))
		.send()
		.await
		.unwrap();
	assert_eq!(response.status().as_u16(), 200);
	assert_eq!(response.headers()["cache-control"], "no-store");
	let deleted = response.json::<Value>().await.unwrap();
	assert_eq!(deleted["state"], "deleted");
	assert_eq!(deleted["revision"], 5);
	let mut scope = service.repository.begin("alpha").await.unwrap();
	let tombstone = scope.get(id).await.unwrap();
	assert_eq!(tombstone.pinned_version, None);
	assert!(tombstone.require_active().is_err());
	scope.commit().await.unwrap();
	assert!(store.secrets.lock().unwrap().is_empty());
	let mut tx = f.database.connection.begin().await.unwrap();
	let decisions = AuthorizationDecision::objects()
		.filter(AuthorizationDecision::field_subject().eq("alice"))
		.all_with_executor(tx.as_mut())
		.await
		.unwrap();
	tx.commit().await.unwrap();
	assert!(
		decisions
			.iter()
			.all(|decision| !decision.action.starts_with("provider_credential"))
	);
	let persisted = all_database_text(&f).await;
	for event in [
		"provider_credential.created",
		"provider_credential.rotated",
		"provider_credential.revoked",
		"provider_credential.deleted",
		"provider_credential.cleanup_completed",
		"provider_credential_binding.updated",
	] {
		assert!(persisted.contains(event), "missing committed event {event}");
	}
	let log = String::from_utf8_lossy(&LOG.get().unwrap().lock().unwrap()).into_owned();
	assert!(log.contains("Provider Credential mutation audit could not be finalized"));
}

#[rstest]
#[tokio::test]
async fn failed_subject_creates_preserve_the_operation_error_and_durable_allow_audit(
	#[future] endpoint: EndpointFixture,
) {
	use aidash_domain::provider_credentials::State;
	use aidash_server::{
		apps::identity::{
			models::AuthorizationDecision, services::provider_credentials::Management,
		},
		authorization::Authorization,
	};
	use reinhardt::db::orm::Model;
	struct RejectedValidator;
	#[async_trait]
	impl KeyValidator for RejectedValidator {
		async fn validate(&self, _: Provider, _: &SecretString) -> Result<Validation> {
			Err(aidash_application::Error::Invalid(
				"Provider Credential validation rejected".into(),
			))
		}
	}
	let f = endpoint.await;
	let bundle = json!({"tenant":"alpha","subjects":{"alice":{"kind":"user"}},"policies":[{"id":"provider-create","effect":"allow","subjects":{"ids":["alice"]},"actions":["provider_credential.create"],"resources":{"kinds":["provider_credential"]}}]});
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
	let issued = assert_json(
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
	let actor = Authorization {
		pool: f.runtime.store.pool.clone(),
	}
	.authenticate(issued["token"].as_str().unwrap())
	.await
	.unwrap();
	let mut seen = std::collections::BTreeSet::new();
	for fail_store in [false, true] {
		let store = Arc::new(FakeStore {
			fail_create: fail_store,
			..Default::default()
		});
		let validator: Arc<dyn KeyValidator> = if fail_store {
			Arc::new(Validator)
		} else {
			Arc::new(RejectedValidator)
		};
		let service = Arc::new(Service {
			repository: Arc::new(NativeRepository {
				pool: f.runtime.store.control_pool.clone(),
				node: f.runtime.store.node_id.clone(),
			}),
			store: store.clone(),
			validator,
			fingerprint_key: "test-fingerprint-key-that-is-at-least-32-bytes".into(),
			max_per_tenant: 20,
		});
		let mut runtime = f.runtime.clone();
		runtime.store.provider_credentials = Some(service.clone());
		let management = Management { runtime };
		let canary = "failed-create-key-material-canary-9a72";
		let error = management
			.create(
				actor.clone(),
				"alpha".into(),
				Provider::Openrouter,
				canary.into(),
			)
			.await
			.unwrap_err();
		if fail_store {
			assert!(
				matches!(error, aidash_server::Error::External(ref message) if message == "Provider Credential Store unavailable")
			);
		} else {
			assert!(
				matches!(error, aidash_server::Error::Invalid(ref message) if message == "Provider Credential validation rejected")
			);
		}
		let mut scope = service.repository.begin("alpha").await.unwrap();
		let rows = scope.list(0, 200).await.unwrap();
		let row = rows.iter().find(|row| !seen.contains(&row.id)).unwrap();
		assert_eq!(
			row.state,
			if fail_store {
				State::Pending
			} else {
				State::Deleted
			}
		);
		assert_eq!(row.revision, if fail_store { 1 } else { 2 });
		assert_eq!(row.pinned_version, None);
		assert!(row.require_active().is_err());
		seen.insert(row.id);
		scope.commit().await.unwrap();
		assert!(store.secrets.lock().unwrap().is_empty());
		let mut tx = f.database.connection.begin().await.unwrap();
		let decisions = AuthorizationDecision::objects()
			.filter(AuthorizationDecision::field_subject().eq("alice"))
			.filter(AuthorizationDecision::field_action().eq("provider_credential.create"))
			.order_by(&["sequence"])
			.all_with_executor(tx.as_mut())
			.await
			.unwrap();
		tx.commit().await.unwrap();
		assert_eq!(decisions.len(), seen.len());
		let decision = decisions.last().unwrap();
		assert_eq!(decision.resource_id, row.id.to_string());
		assert_eq!(decision.decision.0["allowed"], true);
		assert!(!all_database_text(&f).await.contains(canary));
	}
}
