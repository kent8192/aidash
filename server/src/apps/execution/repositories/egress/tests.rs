//! The real gateway authority and meters share PostgreSQL budgets across calls.
#[path = "../../tests/support/settings.rs"]
mod settings;
#[path = "../../tests/support/tenant_roots.rs"]
mod tenant_roots;
use crate::apps::execution::test_database::{DatabaseFixture, database};

use crate::apps::execution::{
	capabilities::{
		models::{
			CoreOperations,
			egress::{Budget, RequestRecord},
		},
		services::sessions,
	},
	repositories::egress::{Gateway, attach},
};
use crate::{
	authorization::{Authorization, access::Access, identity::Actor},
	capabilities::{Profile, RunnerProfile, Runtime},
};
use aidash_application::{
	egress::{Authority, Direction},
	tenancy::TEST_FIXTURE,
};
use chrono::Utc;
use reinhardt::{
	Response, ServerRouter,
	db::orm::{Json, Model},
};
use serde_json::json;
use uuid::Uuid;

const RUNNER_TOKEN_ENV: &str = "AIDASH_TEST_EGRESS_RUNNER_TOKEN";

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn native_gateway_serializes_cumulative_limits_and_refuses_revoked_bindings(
	#[future] database: DatabaseFixture,
) {
	if std::env::var_os(RUNNER_TOKEN_ENV).is_none() {
		// Give only this fixture process a Runner credential without mutating the
		// environment shared by other tests or requiring manual credential setup.
		let status = tokio::process::Command::new(std::env::current_exe().unwrap())
			.args([
				"--exact",
				"egress_tests::native_gateway_serializes_cumulative_limits_and_refuses_revoked_bindings",
				"--nocapture",
			])
			.env(RUNNER_TOKEN_ENV, "native-egress-fixture-token")
			.kill_on_drop(true)
			.status()
			.await
			.unwrap();
		assert!(status.success(), "native gateway fixture failed: {status}");
		return;
	}
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let mut f = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	let started = Utc::now().timestamp_micros() as f64 / 1_000_000.0;
	let runner = reinhardt::test::fixtures::server::test_server_guard(
		ServerRouter::new().handler("/{*rest}", RunnerReply { started }),
	)
	.await;
	let directory = tempfile::tempdir().unwrap();
	let limits = aidash_domain::egress::Limits {
		sent_bytes: 10,
		received_bytes: 12,
		requests: 2,
	};
	let mut profile = Profile {
		admission: true,
		storage: directory.path().to_owned(),
		..Profile::default()
	};
	profile.runner = Some(RunnerProfile {
		endpoint: runner.url.clone(),
		token_env: RUNNER_TOKEN_ENV.into(),
		image: format!("sandbox@sha256:{}", "b".repeat(64)),
		runtime_class: "fixture-runsc".into(),
		namespace: "fixture-sandbox".into(),
		node_guard: None,
		journal: None,
		kubectl: None,
		kubeconfig: None,
		listen_host: None,
		listen_port: None,
	});
	profile.egress = Some(
		crate::apps::execution::capabilities::serializers::core::EgressSettings {
			endpoint: "http://10.96.0.90:8950".into(),
			namespace: None,
			pod_selector: Default::default(),
			listen: "127.0.0.1:0".parse().unwrap(),
			limits,
		},
	);
	f.store.capabilities = Runtime::new(profile).unwrap();
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let scope = tenant_roots::tenant_scope(&f.store, "acme").await;
	let encrypted = std::sync::Arc::new(
		crate::apps::identity::repositories::secret_store::PostgresStore::new(
			f.store.control_pool.clone(),
			"11".repeat(32).into(),
			vec![],
		)
		.await
		.unwrap(),
	);
	let secrets = std::sync::Arc::new(aidash_application::tenant_secrets::Service {
		repository: std::sync::Arc::new(
			crate::apps::identity::repositories::tenant_secrets::NativeRepository {
				pool: f.store.control_pool.clone(),
			},
		),
		store: encrypted.clone(),
		reader: Some(encrypted),
		calls: None,
		fingerprint_key: "tenant-secret-fingerprint-fixture-32-bytes".into(),
		max_per_tenant: 64,
	});
	secrets
		.put(
			"acme",
			"image-key",
			aidash_application::tenant_secrets::Put {
				expected_revision: 0,
				value: Some("live-preview-secret-canary".into()),
				hosts: Some(vec!["api.example.test".into()]),
			},
			"alice",
		)
		.await
		.unwrap();
	f.store.tenant_secrets = Some(secrets);
	f.registry = f.registry.clone().with_tenant_secrets(true);
	let service = serde_json::from_value(
		json!({"id":"images","version":"1.0.0","kind":"external_service",
        "name":{"en":"Images"},"description":{"en":"Fixture service"},
        "config":{"base_url":"https://api.example.test/","methods":["GET","POST"]}}),
	)
	.unwrap();
	f.registry.register(&tenant, service).await.unwrap();
	f.registry
		.register(
			&tenant,
			serde_json::from_value(json!({
				"id":"secured-images","version":"1.0.0","kind":"external_service",
				"name":{"en":"Secured images"},"description":{"en":"Secret-bound fixture service"},
				"config":{"base_url":"https://api.example.test/","methods":["GET"],"secret":"image-key"}
			}))
			.unwrap(),
		)
		.await
		.unwrap();
	for (kind, id, config) in [
		(
			"model",
			"model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"https://model.example.test/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"agent",
			"research",
			json!({"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Fixture",
            "bindings":[{"kind":"external_service","target":{"registry_deployment":f.store.deployment.to_string(),"id":"images","version":"1.0.0"}}, {"kind":"external_service","target":{"registry_deployment":f.store.deployment.to_string(),"id":"secured-images","version":"1.0.0"}}],"remove_default":[]}),
		),
	] {
		f.registry.register(&tenant, serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"config":config})).unwrap()).await.unwrap();
	}
	let agent = f.registry.get(&tenant, "research", "1.0.0").await.unwrap();
	let workspace = f
		.store
		.create_workspace(&scope, "alice", "Gateway", "Cumulative limits")
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			&scope,
			workspace.id,
			&serde_json::from_value(json!({"title":"Gateway","description":"Cumulative limits"}))
				.unwrap(),
			"alice",
			None,
		)
		.await
		.unwrap();
	let run = tenant_roots::admitted_run(&f, "acme", &task, &agent).await;
	let auth = Authorization {
		pool: f.store.pool.clone(),
	};
	let token = auth
		.issue_credential("acme", "alice", 3600, "deployment_administrator")
		.await
		.unwrap()
		.token;
	let Actor::Subject(identity) = auth.authenticate(&token).await.unwrap() else {
		panic!("subject")
	};
	let mut access = Access::begin(&f.store, &identity).await.unwrap();
	access.subjects.push(aidash_domain::qualified_agent(
		&f.store.deployment.node(run.node.clone()),
		&agent.id,
		&agent.version,
	));
	let area = sessions::for_run(&mut access, &run).await.unwrap();
	let operation = Uuid::now_v7();
	let row = CoreOperations::build()
		.id(operation)
		.area_id(area.id)
		.run_id(run.id)
		.credential_id(identity.credential_id)
		.tenant("acme".to_owned())
		.principal(identity.subject.clone())
		.request_key("egress-fixture".to_owned())
		.digest("fixture".to_owned())
		.kind("shell".to_owned())
		.state("running".to_owned())
		.epoch(area.epoch)
		.generation(area.generation)
		.revision(area.revision)
		.policy_revision(access.snapshot.revision)
		.subjects(Json(serde_json::to_value(&access.subjects).unwrap()))
		.input(Json(json!({"seconds":120})))
		.result(Json(json!({})))
		.runner_instance(None)
		.finish();
	CoreOperations::objects()
		.insert_with_executor(&mut **access.tx, &row)
		.await
		.unwrap();
	let mut request = json!({"operation_id":operation,"seconds":120});
	attach(&f.store, &mut access, &mut request).await.unwrap();
	access.finish(Ok(())).await.unwrap();
	let token = request["egress"]["token"].as_str().unwrap();
	let digest = "a".repeat(64);
	assert!(matches!(
		Gateway::new(f.store.clone())
			.admit(
				&format!("{operation}.{}", "a".repeat(43)),
				"images",
				"GET",
				"forged",
				&digest
			)
			.await,
		Err(aidash_application::Error::Forbidden)
	));
	let first = Gateway::new(f.store.clone())
		.admit(token, "images", "POST", "upload", &digest)
		.await
		.unwrap();
	first.meter.charge(Direction::Sent, 6).await.unwrap();
	// A fresh gateway instance sees the same durable operation budget.
	let second = Gateway::new(f.store.clone())
		.admit(token, "images", "GET", "download", &digest)
		.await
		.unwrap();
	let attempts = (0..8).map(|i| {
		let meter = if i % 2 == 0 {
			first.meter.clone()
		} else {
			second.meter.clone()
		};
		async move { meter.charge(Direction::Sent, 1).await }
	});
	let results = futures_util::future::join_all(attempts).await;
	assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 4);
	second.meter.charge(Direction::Received, 12).await.unwrap();
	assert!(first.meter.charge(Direction::Received, 1).await.is_err());
	first.meter.finish(201, 20, None).await.unwrap();
	second.meter.finish(200, 25, None).await.unwrap();
	assert!(matches!(
		Gateway::new(f.store.clone())
			.admit(token, "images", "GET", "third", &digest)
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	let mut tx = f.store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let total = Budget::objects()
		.filter(Budget::field_operation_id().eq(operation))
		.filter(Budget::field_service().eq(""))
		.all_with_executor(&mut *tx)
		.await
		.unwrap()
		.pop()
		.unwrap();
	assert_eq!(
		(total.sent_bytes, total.received_bytes, total.requests),
		(10, 12, 2)
	);
	let records = RequestRecord::objects()
		.filter(RequestRecord::field_operation_id().eq(operation))
		.all_with_executor(&mut *tx)
		.await
		.unwrap();
	assert_eq!(records.len(), 2);
	assert_eq!(records.iter().map(|r| r.sent_bytes).sum::<i64>(), 10);
	assert_eq!(records.iter().map(|r| r.received_bytes).sum::<i64>(), 12);
	assert!(
		records
			.iter()
			.all(|r| r.error.is_none() && r.query_digest == digest && r.status >= 200)
	);
	tx.commit().await.unwrap();
	live_preview_credentials(&f, &identity, &run, area.id).await;
	auth.set_catalog(
		"acme",
		&run.node,
		&aidash_domain::registry::EntityRef {
			id: "images".into(),
			version: "1.0.0".into(),
		},
		1,
		false,
		"deployment_administrator",
	)
	.await
	.unwrap();
	assert!(matches!(
		Gateway::new(f.store.clone())
			.admit(token, "images", "GET", "revoked", &digest)
			.await,
		Err(aidash_application::Error::Forbidden)
	));
	// Metering rechecks Run control, including on an already admitted stream.
	auth.set_catalog(
		"acme",
		&run.node,
		&aidash_domain::registry::EntityRef {
			id: "images".into(),
			version: "1.0.0".into(),
		},
		2,
		true,
		"deployment_administrator",
	)
	.await
	.unwrap();
	first.meter.charge(Direction::Sent, 0).await.unwrap();
	f.store
		.control(&scope, run.id, aidash_domain::RunControlAction::Cancel)
		.await
		.unwrap();
	assert!(matches!(
		first.meter.charge(Direction::Sent, 0).await,
		Err(aidash_application::Error::Forbidden)
	));
}

async fn live_preview_credentials(
	f: &crate::federation::Federation,
	identity: &crate::authorization::identity::SubjectIdentity,
	run: &aidash_domain::Run,
	area: Uuid,
) {
	use crate::apps::execution::repositories::egress::{page_token, session_token};
	use secrecy::ExposeSecret;
	let preview = Uuid::now_v7();
	let now = Utc::now();
	let expires = now + chrono::Duration::hours(2);
	sqlx::query("INSERT INTO core_live_previews (id,tenant,run_id,area_id,revision,port,state,idle_seconds,absolute_seconds,started_at,last_request_at,expires_at) VALUES ($1,'acme',$2,$3,1,3000,'running',900,7200,$4,$4,$5)")
		.bind(preview).bind(run.id).bind(area).bind(now).bind(expires)
		.execute(f.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO egress_sessions (id,tenant,run_id,live_preview_id,principal,credential_id,expires_at,max_sent_bytes,max_received_bytes,max_requests) VALUES ($1,'acme',$2,$1,'alice',$3,$4,100,100,10)")
		.bind(preview).bind(run.id).bind(identity.credential_id).bind(expires)
		.execute(f.store.pool.fixture_driver()).await.unwrap();
	let pod = session_token(preview, expires, &f.store).unwrap();
	let page = page_token(preview, expires, &f.store).unwrap();
	let gateway = Gateway::new(f.store.clone());
	let digest = "b".repeat(64);
	// A Pod cannot change its credential's kind to impersonate the page path,
	// and a page cannot turn its credential into one that attaches the Secret.
	for forged in [
		pod.replacen("session:", "page:", 1),
		page.replacen("page:", "session:", 1),
	] {
		assert!(matches!(
			gateway
				.admit(&forged, "secured-images", "GET", "/", &digest)
				.await,
			Err(aidash_application::Error::Forbidden)
		));
	}
	let pod = gateway
		.admit(&pod, "secured-images", "GET", "/", &digest)
		.await
		.unwrap();
	assert_eq!(
		pod.credential.as_ref().unwrap().expose_secret(),
		"live-preview-secret-canary"
	);
	let page = gateway
		.admit(&page, "secured-images", "GET", "/", &digest)
		.await
		.unwrap();
	assert!(
		page.credential.is_none(),
		"the page reaches the same service without its Tenant Secret"
	);
	for (admission, via, host) in [
		(&pod, "pod", Some("api.example.test")),
		(&page, "page", None),
	] {
		admission.meter.finish(200, 1, None).await.unwrap();
		let (recorded, secret_host): (String, Option<String>) =
			sqlx::query_as("SELECT via, secret_host FROM egress_requests WHERE id = $1")
				.bind(admission.request)
				.fetch_one(f.store.pool.fixture_driver())
				.await
				.unwrap();
		assert_eq!(recorded, via);
		// Only the request that carried the Tenant Secret names where it went.
		assert_eq!(secret_host.as_deref(), host);
	}
	// A brokered preview mints once per session and records only the pinned identity.
	let existing = f.store.tenant_secrets.as_ref().unwrap();
	let calls = std::sync::Arc::new(RecordedCalls::default());
	let mut store = f.store.clone();
	store.tenant_secrets = Some(std::sync::Arc::new(
		aidash_application::tenant_secrets::Service {
			repository: existing.repository.clone(),
			store: existing.store.clone(),
			reader: None,
			calls: Some(calls.clone()),
			fingerprint_key: existing.fingerprint_key.clone(),
			max_per_tenant: existing.max_per_tenant,
		},
	));
	let gateway = Gateway::new(store);
	let token = session_token(preview, expires, &f.store).unwrap();
	for _ in 0..2 {
		let admitted = gateway
			.admit(&token, "secured-images", "GET", "/", &digest)
			.await
			.unwrap();
		assert!(admitted.credential.is_none());
		assert_eq!(
			admitted.route.as_ref().unwrap().token.expose_secret(),
			"fixture-route-token"
		);
		admitted.meter.finish(200, 1, None).await.unwrap();
		let row: (Option<Uuid>, Option<String>, Option<String>) = sqlx::query_as(
			"SELECT secret_id, secret_version, secret_host FROM egress_requests WHERE id = $1",
		)
		.bind(admitted.request)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
		assert!(row.0.is_some());
		assert_eq!(row.1.as_deref(), Some("1"));
		assert_eq!(row.2.as_deref(), Some("api.example.test"));
	}
	let grants = calls.0.lock().unwrap();
	assert_eq!(grants.len(), 1);
	assert_eq!(grants[0].tenant, "acme");
	assert_eq!(grants[0].destination.origin, "https://api.example.test");
	assert_eq!(grants[0].destination.path_prefix, "/");
	assert_eq!(grants[0].methods, vec!["GET"]);
	assert!(
		matches!(&grants[0].subject, aidash_capability::tenant_secret::SecretSubject::Operation { operation, tenant } if *operation == preview && tenant == "acme")
	);
}

struct RunnerReply {
	started: f64,
}
#[async_trait::async_trait]
impl reinhardt::Handler for RunnerReply {
	async fn handle(&self, request: reinhardt::Request) -> reinhardt::http::ViewResult<Response> {
		assert_eq!(
			request.headers["authorization"].to_str().unwrap(),
			format!("Bearer {}", std::env::var(RUNNER_TOKEN_ENV).unwrap())
		);
		Ok(Response::ok()
			.with_json(&json!({"status":"running","started_at":self.started}))
			.unwrap())
	}
}

#[derive(Default)]
struct RecordedCalls(std::sync::Mutex<Vec<aidash_application::tenant_secrets::SecretCallGrant>>);
#[async_trait::async_trait]
impl aidash_application::tenant_secrets::SecretCalls for RecordedCalls {
	async fn route(
		&self,
		grant: aidash_application::tenant_secrets::SecretCallGrant,
	) -> aidash_application::Result<aidash_application::tenant_secrets::SecretRoute> {
		self.0.lock().unwrap().push(grant);
		Ok(aidash_application::tenant_secrets::SecretRoute {
			base: "https://broker.example/v1/forward".parse().unwrap(),
			token: "fixture-route-token".into(),
		})
	}
}
