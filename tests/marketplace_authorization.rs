mod common;
use aidash::{
	api,
	authorization::{Authorization, policy::PolicyBundle},
	federation::Federation,
	registry::{EntityRef, Entry},
};
use axum::Router;
use common::{TestEnvironment, request, test_environment};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

// Advisory locks are database-wide while the shared test service isolates
// schemas. Serialize fixtures so intentional barriers cannot block another test
// holding a different half of the lock order; requests inside each test race.
static FIXTURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn tool(id: &str) -> Entry {
	serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"tool","name":{"en":id},"description":{"en":"Local tool"},"config":{"transport":"http","endpoint":"http://localhost:9/original","replay":"read_only","credential_env":null}})).unwrap()
}
fn bundle(tenant: &str, kind: &str) -> Value {
	json!({"tenant":tenant,"subjects":{"viewer":{"kind":kind,"roles":["manager"]}},"roles":{"manager":{"inherits":["reader"]},"reader":{}},"policies":[{"id":"all","effect":"allow","subjects":{"roles":["reader"]},"actions":["*"],"resources":{"kinds":["*"]}}]})
}
async fn policy(f: &Federation, tenant: &str, revision: i64, value: Value) {
	Authorization {
		pool: f.store.pool.clone(),
	}
	.replace(
		tenant,
		revision,
		serde_json::from_value::<PolicyBundle>(value).unwrap(),
		"operator",
	)
	.await
	.unwrap();
}
async fn token(f: &Federation, tenant: &str, kind: &str) -> String {
	policy(f, tenant, 0, bundle(tenant, kind)).await;
	Authorization {
		pool: f.store.pool.clone(),
	}
	.issue_credential(tenant, "viewer", 3600, "operator")
	.await
	.unwrap()
	.token
}
async fn enable(app: &Router, f: &Federation) {
	let _ = tracing_subscriber::fmt()
		.with_env_filter("aidash=error")
		.with_test_writer()
		.try_init();
	let result = request(
		app,
		&f.config.api_token,
		"PUT",
		"/api/marketplace/compatibility",
		json!({"enabled":true,"expected_revision":1,"compatible_instances_confirmed":true}),
	)
	.await;
	assert_eq!(result.0, 200, "{result:?}");
}
async fn approve(f: &Federation, tenant: &str, entry: &EntityRef) {
	Authorization {
		pool: f.store.pool.clone(),
	}
	.set_catalog(tenant, entry, 0, true, "operator")
	.await
	.unwrap();
}
async fn publish(app: &Router, token: &str, id: &str) -> Value {
	let result=request(app,token,"POST","/api/marketplace/packages",json!({"source":reference(id),"package_id":"shared-name","author":"display only","idempotency_key":Uuid::new_v4()})).await;
	assert_eq!(result.0, 200, "{result:?}");
	result.1
}
fn install_input(package: &Value) -> Value {
	json!({"digest":package["digest"],"config":{},"idempotency_key":Uuid::new_v4()})
}
async fn install(app: &Router, token: &str, package: &Value, input: &Value) -> (u16, Value) {
	request(
		app,
		token,
		"POST",
		&format!(
			"/api/marketplace/packages/{}/install",
			package["key"].as_str().unwrap()
		),
		input.clone(),
	)
	.await
}
#[allow(clippy::too_many_arguments)] // Mirrors the independent approval and pointer preconditions.
async fn activate(
	app: &Router,
	f: &Federation,
	tenant: &str,
	installation: &Value,
	revision: i64,
	pointer: i64,
	catalog: i64,
	enabled: bool,
) -> (u16, Value) {
	request(app,&f.config.api_token,"POST",&format!("/api/marketplace/installations/{}/activation",installation["installation"]["id"].as_str().unwrap()),json!({"tenant":tenant,"revision":revision,"expected_activation_revision":pointer,"expected_catalog_revision":catalog,"enabled":enabled})).await
}
fn deny(mut value: Value, actions: Value) -> Value {
	value["policies"].as_array_mut().unwrap().push(json!({"id":"deny","effect":"deny","subjects":{"any":true},"actions":actions,"resources":{"kinds":["*"]}}));
	value
}

#[rstest::rstest]
#[case("user")]
#[case("agent")]
#[case("node")]
#[case("service")]
#[tokio::test]
async fn tenant_publication_pending_installation_and_revisions(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] kind: &str,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", kind).await;
	let b = token(&f, "b", "user").await;
	assert_eq!(
		request(&app, &a, "GET", "/api/marketplace/packages", Value::Null)
			.await
			.0,
		403
	);
	enable(&app, &f).await;
	f.registry.register(tool("source-a")).await.unwrap();
	f.registry.register(tool("source-b")).await.unwrap();
	approve(&f, "a", &reference("source-a")).await;
	approve(&f, "b", &reference("source-b")).await;
	let pa = publish(&app, &a, "source-a").await;
	let pb = publish(&app, &b, "source-b").await;
	assert_ne!(pa["key"], pb["key"]);
	let list = request(&app, &a, "GET", "/api/marketplace/packages", Value::Null).await;
	assert_eq!(list.0, 200);
	assert_eq!(list.1.as_array().unwrap().len(), 1);
	assert_eq!(list.1[0]["owner_tenant"], "a");
	let missing = request(
		&app,
		&a,
		"GET",
		"/api/marketplace/packages/missing",
		Value::Null,
	)
	.await;
	assert_eq!(
		request(
			&app,
			&a,
			"GET",
			&format!("/api/marketplace/packages/{}", pb["key"].as_str().unwrap()),
			Value::Null
		)
		.await,
		missing
	);
	let input = install_input(&pa);
	let installed = install(&app, &a, &pa, &input).await;
	assert_eq!(installed.0, 200, "{installed:?}");
	assert_eq!(installed.1["approved"], false);
	assert_eq!(
		install(&api::router(f.clone()), &a, &pa, &input).await,
		installed
	);
	let id = installed.1["installation"]["id"].as_str().unwrap();
	let entry: Entry = serde_json::from_value(installed.1["entry"].clone()).unwrap();
	assert_eq!(entry.installation.as_ref().unwrap().tenant, "a");
	// Another tenant's document must never be loaded or decoded by this list.
	// Its indexed owner remains enough to isolate even damaged foreign data.
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("marketplace_installations"))
			.columns(["key", "document", "tenant", "package_key"].map(Alias::new))
			.values_panic([
				Expr::value("foreign-damaged-installation"),
				Expr::value(json!({"damaged":true})),
				Expr::value("b"),
				Expr::value("foreign-package"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let listed = request(
		&app,
		&a,
		"GET",
		"/api/marketplace/installations",
		Value::Null,
	)
	.await;
	assert_eq!(
		listed.0, 200,
		"foreign data affected the tenant list: {listed:?}"
	);
	assert_eq!(listed.1.as_array().unwrap().len(), 1);
	assert_eq!(listed.1[0]["installation"]["id"], id);
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			&format!("/api/marketplace/installations/{id}"),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert!(
		Authorization {
			pool: f.store.pool.clone()
		}
		.set_catalog(
			"b",
			&EntityRef {
				id: entry.id.clone(),
				version: entry.version.clone()
			},
			0,
			true,
			"operator"
		)
		.await
		.is_err()
	);
	let catalog = request(&app, &a, "GET", "/api/registry", Value::Null).await;
	assert!(!catalog.1.to_string().contains(&entry.id));
	let active = activate(&app, &f, "a", &installed.1, 1, 0, 0, true).await;
	assert_eq!(active.0, 200, "{active:?}");
	for enabled in [false, true] {
		assert!(
			Authorization {
				pool: f.store.pool.clone()
			}
			.set_catalog(
				"a",
				&EntityRef {
					id: entry.id.clone(),
					version: entry.version.clone()
				},
				1,
				enabled,
				"operator"
			)
			.await
			.is_err(),
			"projection catalog writes must use activation"
		);
	}
	let change = json!({"expected_revision":1,"config":{"endpoint":"http://localhost:9/reconfigured"},"idempotency_key":Uuid::new_v4()});
	let changed = request(
		&app,
		&a,
		"POST",
		&format!("/api/marketplace/installations/{id}"),
		change.clone(),
	)
	.await;
	assert_eq!(changed.0, 200, "{changed:?}");
	assert_eq!(changed.1["revision"], 2);
	assert_eq!(changed.1["installation"]["active_revision"], 1);
	assert_eq!(changed.1["approved"], false);
	assert_eq!(
		request(
			&app,
			&a,
			"POST",
			&format!("/api/marketplace/installations/{id}"),
			change
		)
		.await,
		changed
	);
	assert_eq!(
		f.registry
			.get(&entry.id, &entry.version)
			.await
			.unwrap()
			.config["endpoint"],
		"http://localhost:9/original"
	);
	let current = request(&app, &a, "GET", "/api/registry", Value::Null).await;
	assert!(current.1.to_string().contains(&entry.id));
	assert!(
		!current
			.1
			.to_string()
			.contains(changed.1["entry"]["id"].as_str().unwrap())
	);
	let result = activate(&app, &f, "a", &installed.1, 2, 1, 0, true).await;
	assert_eq!(result.0, 200, "{result:?}");
	let current = request(&app, &a, "GET", "/api/registry", Value::Null).await;
	assert!(!current.1.to_string().contains(&entry.id));
	assert!(
		current
			.1
			.to_string()
			.contains(changed.1["entry"]["id"].as_str().unwrap())
	);
	// Exact old reads survive activation and disappear only on explicit revocation.
	assert_eq!(
		request(
			&app,
			&a,
			"GET",
			&format!("/api/registry/{}/{}", entry.id, entry.version),
			Value::Null
		)
		.await
		.0,
		200
	);
	assert_eq!(
		activate(&app, &f, "a", &installed.1, 1, 2, 1, false)
			.await
			.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&a,
			"GET",
			&format!("/api/registry/{}/{}", entry.id, entry.version),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		activate(&app, &f, "a", &installed.1, 2, 3, 1, false)
			.await
			.0,
		200
	);
	assert!(
		!request(&app, &a, "GET", "/api/registry", Value::Null)
			.await
			.1
			.to_string()
			.contains("mkt-")
	);
	let mut replay_input = input.clone();
	replay_input["config"] = json!({"endpoint":"http://localhost:9/other"});
	assert_eq!(install(&app, &a, &pa, &replay_input).await.0, 409);
	policy(
		&f,
		"a",
		1,
		deny(
			bundle("a", kind),
			json!(["marketplace.read", "marketplace.install"]),
		),
	)
	.await;
	assert_eq!(install(&app, &a, &pa, &input).await.0, 403);
	// Local committed management/configuration does not require distribution.
	assert_eq!(
		request(
			&app,
			&a,
			"GET",
			&format!("/api/marketplace/installations/{id}"),
			Value::Null
		)
		.await
		.0,
		200
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn hidden_typed_dependency_and_denial_leave_no_installation(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let b = token(&f, "b", "user").await;
	enable(&app, &f).await;
	let model:Entry=serde_json::from_value(json!({"id":"hidden-model","version":"1.0.0","kind":"model","name":{"en":"Hidden model"},"description":{"en":"private"},"config":{"provider":"openrouter","model_id":"fixture/model","endpoint":"http://localhost:9","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	f.registry.register(model).await.unwrap();
	let agent:Entry=serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Visible agent"},"description":{"en":"Public summary"},"config":{"model":reference("hidden-model"),"instructions":"hello"}})).unwrap();
	f.registry.register(agent).await.unwrap();
	approve(&f, "a", &reference("hidden-model")).await;
	approve(&f, "a", &reference("agent")).await;
	let package = publish(&app, &a, "agent").await;
	let key = package["key"].as_str().unwrap();
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&format!("/api/marketplace/packages/{key}/audience"),
			json!({"expected_revision":1,"tenants":["a","b"]})
		)
		.await
		.0,
		200
	);
	let list = request(&app, &b, "GET", "/api/marketplace/packages", Value::Null).await;
	assert_eq!(list.0, 200);
	assert_eq!(list.1.as_array().unwrap().len(), 1);
	assert!(!list.1.to_string().contains("hidden-model"));
	assert_eq!(list.1[0]["actions"], json!([]));
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			"/api/marketplace/packages?q=hidden-model",
			Value::Null
		)
		.await
		.1,
		json!([])
	);
	let denied = request(
		&app,
		&b,
		"GET",
		&format!("/api/marketplace/packages/{key}"),
		Value::Null,
	)
	.await;
	assert_eq!(
		denied,
		request(
			&app,
			&b,
			"GET",
			"/api/marketplace/packages/missing",
			Value::Null
		)
		.await
	);
	let input = install_input(&package);
	assert_eq!(install(&app, &b, &package, &input).await.0, 403);
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			"/api/marketplace/installations",
			Value::Null
		)
		.await
		.1,
		json!([])
	);
	assert!(
		!f.registry
			.list(&Default::default())
			.await
			.unwrap()
			.iter()
			.any(|e| e.installation.is_some())
	);
	approve(&f, "b", &reference("hidden-model")).await;
	let mut wrong = input.clone();
	wrong["digest"] = json!("wrong");
	assert_eq!(install(&app, &b, &package, &wrong).await.0, 409);
	let installed = install(&app, &b, &package, &input).await;
	assert_eq!(installed.0, 200, "{installed:?}");
	// A deny overrides inherited role permission and ownership at each surface.
	policy(
		&f,
		"a",
		1,
		deny(
			bundle("a", "user"),
			json!(["marketplace.share", "registry.export"]),
		),
	)
	.await;
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&format!("/api/marketplace/packages/{key}/audience"),
			json!({"expected_revision":2,"tenants":[]})
		)
		.await
		.0,
		403
	);
	assert_eq!(request(&app,&a,"POST","/api/marketplace/packages",json!({"source":reference("agent"),"package_id":"second","author":"a","idempotency_key":Uuid::new_v4()})).await.0,403);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn browse_pages_hidden_versions_before_returning_a_visible_package(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	f.registry.register(tool("paged-source")).await.unwrap();
	approve(&f, "a", &reference("paged-source")).await;
	let published = publish(&app, &a, "paged-source").await;
	let source_key = published["key"].as_str().unwrap();
	let (document, source_content): (Value, String) = sqlx::query_as(
		&Query::select()
			.columns(["document", "source_content"].map(Alias::new))
			.from(Alias::new("marketplace_versions"))
			.and_where(Expr::cust("key=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(source_key)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	let source_id = document["source"]["id"].as_str().unwrap();
	let source_version = document["source"]["version"].as_str().unwrap();
	for i in 0..64 {
		let key = format!("!hidden-{i:03}");
		let package_id = format!("hidden-{i:03}");
		let mut hidden = document.clone();
		hidden["key"] = json!(key);
		hidden["package_id"] = json!(package_id);
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("marketplace_versions"))
				.columns(
					[
						"key",
						"document",
						"repository",
						"owner",
						"package_id",
						"version",
						"kind",
						"source_id",
						"source_version",
						"source_content",
					]
					.map(Alias::new),
				)
				.values_panic((1..=10).map(|i| Expr::cust(format!("${i}"))))
				.to_string(PostgresQueryBuilder),
		)
		.bind(key)
		.bind(hidden)
		.bind(&f.store.node_id)
		.bind("a")
		.bind(package_id)
		.bind("1.0.0")
		.bind("tool")
		.bind(source_id)
		.bind(source_version)
		.bind(&source_content)
		.execute(&f.store.pool)
		.await
		.unwrap();
	}
	let (status, page) = request(
		&app,
		&a,
		"GET",
		"/api/marketplace/packages?q=shared-name&limit=1",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(page.as_array().unwrap().len(), 1);
	assert_eq!(page[0]["key"], source_key);
	let audit: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("events"))
			.and_where(Expr::cust("kind='marketplace.audit'"))
			.order_by(Alias::new("sequence"), sea_orm::sea_query::Order::Desc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(audit["operation"], "marketplace.browse");
	assert_eq!(audit["resource"]["query"], "shared-name");
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn live_redistribution_consent_retains_local_copies_and_export_bytes(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let b = token(&f, "b", "user").await;
	let c = token(&f, "c", "user").await;
	enable(&app, &f).await;
	f.registry.register(tool("original")).await.unwrap();
	approve(&f, "a", &reference("original")).await;
	let original = publish(&app, &a, "original").await;
	let key = original["key"].as_str().unwrap();
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&format!("/api/marketplace/packages/{key}/audience"),
			json!({"expected_revision":1,"tenants":["a","b"]})
		)
		.await
		.0,
		200
	);
	let mut input = install_input(&original);
	input["config"] = json!({"endpoint":"http://localhost:9/tenant-secret-setting"});
	let installed = install(&app, &b, &original, &input).await;
	assert_eq!(installed.0, 200, "{installed:?}");
	assert_eq!(
		activate(&app, &f, "b", &installed.1, 1, 0, 0, true).await.0,
		200
	);
	let source = EntityRef {
		id: installed.1["entry"]["id"].as_str().unwrap().into(),
		version: "1.0.0".into(),
	};
	let publication = json!({"source":source,"package_id":"downstream","author":"B","idempotency_key":Uuid::new_v4()});
	assert_eq!(
		request(
			&app,
			&b,
			"POST",
			"/api/marketplace/packages",
			publication.clone()
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&format!("/api/marketplace/packages/{key}/consents/b"),
			json!({"expected_revision":0,"tenants":["b","c"]})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&a,
			"GET",
			&format!("/api/marketplace/packages/{key}/consents/b"),
			Value::Null,
		)
		.await
		.1,
		json!({"revision":1,"tenants":["b","c"]}),
	);
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			&format!("/api/marketplace/packages/{key}/consents/b"),
			Value::Null,
		)
		.await
		.0,
		403,
	);
	let downstream = request(
		&app,
		&b,
		"POST",
		"/api/marketplace/packages",
		publication.clone(),
	)
	.await;
	assert_eq!(downstream.0, 200, "{downstream:?}");
	let child = downstream.1["key"].as_str().unwrap();
	assert_eq!(
		request(
			&app,
			&b,
			"PUT",
			&format!("/api/marketplace/packages/{child}/audience"),
			json!({"expected_revision":1,"tenants":["b","c"]})
		)
		.await
		.0,
		200
	);
	let detail = request(
		&app,
		&c,
		"GET",
		&format!("/api/marketplace/packages/{child}"),
		Value::Null,
	)
	.await;
	assert_eq!(detail.0, 200, "{detail:?}");
	assert_eq!(
		detail.1["manifest"]["entity"]["config"]["endpoint"],
		"http://localhost:9/original"
	);
	assert!(!detail.1.to_string().contains("tenant-secret-setting"));
	let local_c = install(&app, &c, &downstream.1, &install_input(&downstream.1)).await;
	assert_eq!(local_c.0, 200, "{local_c:?}");
	// The acquisition audience remains live in known downstream provenance too.
	let audience_path = format!("/api/marketplace/packages/{key}/audience");
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&audience_path,
			json!({"expected_revision":2,"tenants":["a"]})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&c,
			"GET",
			&format!("/api/marketplace/packages/{child}"),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&audience_path,
			json!({"expected_revision":3,"tenants":["a","b"]})
		)
		.await
		.0,
		200
	);
	// Withdrawing the original consent blocks already-published child versions.
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&format!("/api/marketplace/packages/{key}/consents/b"),
			json!({"expected_revision":1,"tenants":[]})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, &c, "GET", "/api/marketplace/packages", Value::Null)
			.await
			.1,
		json!([])
	);
	assert_eq!(
		request(
			&app,
			&c,
			"GET",
			&format!("/api/marketplace/packages/{child}"),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(&app, &b, "POST", "/api/marketplace/packages", publication)
			.await
			.0,
		403
	);
	let local_id = local_c.1["installation"]["id"].as_str().unwrap();
	assert_eq!(
		request(
			&app,
			&c,
			"GET",
			&format!("/api/marketplace/installations/{local_id}"),
			Value::Null
		)
		.await
		.0,
		200
	);
	assert_eq!(request(&app,&c,"POST",&format!("/api/marketplace/installations/{local_id}"),json!({"expected_revision":1,"config":{"endpoint":"http://localhost:9/retained"},"idempotency_key":Uuid::new_v4()})).await.0,200);
	// Legacy paths remain operator-only, and unknown event families leak nothing.
	assert_eq!(
		request(&app, &b, "GET", "/api/marketplace", Value::Null)
			.await
			.0,
		403
	);
	let events = request(&app, &c, "GET", "/api/events", Value::Null).await;
	assert!(!events.1.to_string().contains(key));
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn explicit_legacy_adoption_and_mixed_writer_fence(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	let legacy = aidash::registry::Package {
		entity: tool("mkt-legacy"),
		author: "mkt-legacy".into(),
		permissions: vec![],
		dependencies: vec![],
	};
	let package = f.registry.publish(&f.store.pool, legacy).await.unwrap();
	f.registry
		.install(
			&f.store.pool,
			"mkt-legacy",
			"1.0.0",
			&package.digest,
			json!({"endpoint":"http://localhost:9/legacy"}),
		)
		.await
		.unwrap();
	assert_eq!(
		request(&app, &a, "GET", "/api/marketplace/packages", Value::Null)
			.await
			.1,
		json!([])
	);
	let input =
		json!({"tenant":"a","source":reference("mkt-legacy"),"idempotency_key":Uuid::new_v4()});
	assert_eq!(
		request(
			&app,
			&a,
			"POST",
			"/api/marketplace/adoptions",
			input.clone()
		)
		.await
		.0,
		403
	);
	let adopted = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/marketplace/adoptions",
		input.clone(),
	)
	.await;
	assert_eq!(adopted.0, 200, "{adopted:?}");
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/marketplace/adoptions",
			input
		)
		.await,
		adopted
	);
	let id = adopted.1["id"].as_str().unwrap();
	let installed = request(
		&app,
		&a,
		"GET",
		&format!("/api/marketplace/installations/{id}"),
		Value::Null,
	)
	.await;
	assert_eq!(installed.1["approved"], false);
	assert_eq!(
		installed.1["entry"]["config"]["endpoint"],
		"http://localhost:9/legacy"
	);
	f.registry
		.install(
			&f.store.pool,
			"mkt-legacy",
			"1.0.0",
			&package.digest,
			json!({"endpoint":"http://localhost:9/changed"}),
		)
		.await
		.unwrap();
	assert_eq!(
		request(
			&app,
			&a,
			"GET",
			&format!("/api/marketplace/installations/{id}"),
			Value::Null
		)
		.await
		.1["entry"]["config"]["endpoint"],
		"http://localhost:9/legacy"
	);
	let entry = installed.1["entry"]["id"].as_str().unwrap();
	let write = sqlx::query(
		&Query::insert()
			.into_table(Alias::new("installations"))
			.columns(["id", "version", "config"].map(Alias::new))
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("'1.0.0'"),
				Expr::cust("'{}'::jsonb"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(entry)
	.execute(&f.store.pool)
	.await;
	assert!(
		write.is_err(),
		"old global overlay writer accepted a tenant projection"
	);
	let mut replacement = installed.1["entry"].clone();
	replacement["installation"] = json!({"contract":1,"tenant":"b","installation":id,"revision":1});
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/registry",
			replacement
		)
		.await
		.0,
		400
	);
	let mut ordinary = installed.1["entry"].clone();
	ordinary.as_object_mut().unwrap().remove("installation");
	ordinary["id"] = json!("known-copy");
	assert_eq!(
		request(&app, &f.config.api_token, "POST", "/api/registry", ordinary)
			.await
			.0,
		200
	);
	approve(&f, "a", &reference("known-copy")).await;
	// Same-tenant known content remains linked to its source, including copies.
	let copy = publish(&app, &a, "known-copy").await;
	assert_ne!(copy["key"], adopted.1["package_key"]);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn adoption_freezes_the_effective_dependency_graph(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	for id in ["legacy-model-original", "legacy-model-overlay"] {
		let model: Entry = serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":"model","name":{"en":id},"description":{"en":"fixture"},"config":{"provider":"openrouter","model_id":"fixture/model","endpoint":"http://localhost:9","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
		f.registry.register(model).await.unwrap();
	}
	approve(&f, "a", &reference("legacy-model-overlay")).await;
	let agent: Entry = serde_json::from_value(json!({"id":"legacy-agent","version":"1.0.0","kind":"agent","name":{"en":"Legacy agent"},"description":{"en":"fixture"},"config":{"model":reference("legacy-model-original"),"instructions":"Original"}})).unwrap();
	let legacy = aidash::registry::Package {
		entity: agent,
		author: "legacy".into(),
		permissions: vec![],
		dependencies: vec![reference("legacy-model-original")],
	};
	let package = f.registry.publish(&f.store.pool, legacy).await.unwrap();
	f.registry
		.install(
			&f.store.pool,
			"legacy-agent",
			"1.0.0",
			&package.digest,
			json!({"model":reference("legacy-model-overlay")}),
		)
		.await
		.unwrap();
	let adopted = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/marketplace/adoptions",
		json!({"tenant":"a","source":reference("legacy-agent"),"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(adopted.0, 200, "{adopted:?}");
	let id = adopted.1["id"].as_str().unwrap();
	let installed = request(
		&app,
		&a,
		"GET",
		&format!("/api/marketplace/installations/{id}"),
		Value::Null,
	)
	.await;
	assert_eq!(installed.0, 200, "{installed:?}");
	assert_eq!(
		installed.1["entry"]["config"]["model"],
		json!(reference("legacy-model-overlay"))
	);
	assert_eq!(
		installed.1["dependencies"],
		json!([reference("legacy-model-overlay")])
	);
	let key = adopted.1["package_key"].as_str().unwrap();
	let detail = request(
		&app,
		&a,
		"GET",
		&format!("/api/marketplace/packages/{key}"),
		Value::Null,
	)
	.await;
	assert_eq!(detail.0, 200, "{detail:?}");
	assert_eq!(
		detail.1["manifest"]["entity"]["config"]["model"],
		json!(reference("legacy-model-overlay"))
	);
	common::cleanup(f, &url, &schema).await;
}

async fn wait_for_lock(f: &Federation, schema: &str, pattern: &str) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	for _ in 0..500 {
		let count: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("count(*)"))
				.from(Alias::new("pg_stat_activity"))
				.and_where(Expr::cust(
					"application_name=$1 AND wait_event_type='Lock' AND query LIKE $2",
				))
				.to_string(PostgresQueryBuilder),
		)
		.bind(schema)
		.bind(pattern)
		.fetch_one(&f.store.pool)
		.await
		.unwrap();
		if count > 0 {
			return;
		}
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}
	panic!("request did not reach its deterministic database barrier");
}

#[rstest::rstest]
#[tokio::test]
async fn audience_revocation_and_install_commit_have_a_durable_order(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let b = token(&f, "b", "user").await;
	enable(&app, &f).await;
	f.registry.register(tool("racing")).await.unwrap();
	approve(&f, "a", &reference("racing")).await;
	let package = publish(&app, &a, "racing").await;
	let key = package["key"].as_str().unwrap();
	let audience_path = format!("/api/marketplace/packages/{key}/audience");
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&audience_path,
			json!({"expected_revision":1,"tenants":["a","b"]})
		)
		.await
		.0,
		200
	);
	// Revocation wins: hold the same distribution lock before the installing
	// request reaches it, change the audience, then commit the barrier.
	let mut barrier = f.store.pool.begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.expr(Expr::cust("pg_advisory_xact_lock(74003201)"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *barrier)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("marketplace_audiences"))
			.value(Alias::new("document"), Expr::cust("$2"))
			.and_where(Expr::cust("key=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(key)
	.bind(json!({"revision":3,"tenants":["a"]}))
	.execute(&mut *barrier)
	.await
	.unwrap();
	let (app2, b2, package2) = (app.clone(), b.clone(), package.clone());
	let blocked =
		tokio::spawn(
			async move { install(&app2, &b2, &package2, &install_input(&package2)).await },
		);
	wait_for_lock(&f, &schema, "%74003201%").await;
	barrier.commit().await.unwrap();
	assert_eq!(blocked.await.unwrap().0, 403);
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			"/api/marketplace/installations",
			Value::Null
		)
		.await
		.1,
		json!([])
	);
	// Installation wins: block its final success-event insertion after all
	// protected reads/writes, then queue a revoker on the distribution lock.
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&audience_path,
			json!({"expected_revision":3,"tenants":["a","b"]})
		)
		.await
		.0,
		200
	);
	let mut barrier = f.store.pool.begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.expr(Expr::cust("pg_advisory_xact_lock(71003201)"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *barrier)
	.await
	.unwrap();
	let (app2, b2, package2) = (app.clone(), b.clone(), package.clone());
	let allowed =
		tokio::spawn(
			async move { install(&app2, &b2, &package2, &install_input(&package2)).await },
		);
	wait_for_lock(&f, &schema, "%71003201%").await;
	let (app2, a2) = (app.clone(), a.clone());
	let revoked = tokio::spawn(async move {
		request(
			&app2,
			&a2,
			"PUT",
			&audience_path,
			json!({"expected_revision":4,"tenants":["a"]}),
		)
		.await
	});
	wait_for_lock(&f, &schema, "%74003201%").await;
	barrier.commit().await.unwrap();
	let installed = allowed.await.unwrap();
	assert_eq!(installed.0, 200, "{installed:?}");
	assert_eq!(revoked.await.unwrap().0, 200);
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			&format!("/api/marketplace/packages/{key}"),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			"/api/marketplace/installations",
			Value::Null
		)
		.await
		.1
		.as_array()
		.unwrap()
		.len(),
		1
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case("credential")]
#[case("policy")]
#[tokio::test]
async fn revoked_authority_cannot_commit_a_waiting_installation(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] authority: &str,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	f.registry.register(tool("revocation")).await.unwrap();
	approve(&f, "a", &reference("revocation")).await;
	let package = publish(&app, &a, "revocation").await;
	let mut barrier = f.store.pool.begin().await.unwrap();
	let query = if authority == "credential" {
		Query::update()
			.table(Alias::new("authorization_credentials"))
			.value(Alias::new("revoked_at"), Expr::cust("clock_timestamp()"))
			.and_where(Expr::cust("tenant='a'"))
			.to_string(PostgresQueryBuilder)
	} else {
		Query::update()
			.table(Alias::new("authorization_bundles"))
			.value(Alias::new("document"), Expr::cust("$1"))
			.and_where(Expr::cust("tenant='a'"))
			.to_string(PostgresQueryBuilder)
	};
	if authority == "credential" {
		sqlx::query(&query).execute(&mut *barrier).await.unwrap();
	} else {
		sqlx::query(&query)
			.bind(deny(bundle("a", "user"), json!(["marketplace.install"])))
			.execute(&mut *barrier)
			.await
			.unwrap();
	}
	let (app2, a2, package2) = (app.clone(), a.clone(), package.clone());
	let blocked =
		tokio::spawn(
			async move { install(&app2, &a2, &package2, &install_input(&package2)).await },
		);
	wait_for_lock(
		&f,
		&schema,
		if authority == "credential" {
			"%authorization_credentials%"
		} else {
			"%authorization_bundles%"
		},
	)
	.await;
	barrier.commit().await.unwrap();
	let denied = blocked.await.unwrap();
	assert!(matches!(denied.0, 401 | 403), "{denied:?}");
	assert!(
		!f.registry
			.list(&Default::default())
			.await
			.unwrap()
			.iter()
			.any(|e| e.installation.is_some())
	);
	assert!(
		!f.store
			.events(0, None, 1000)
			.await
			.unwrap()
			.iter()
			.any(|e| e.kind == "marketplace.installed")
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn new_runs_select_active_revision_and_restarted_runs_keep_exact_old_reference(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	let model:Entry=serde_json::from_value(json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"fixture"},"config":{"provider":"openrouter","model_id":"fixture/model","endpoint":"http://localhost:9","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	f.registry.register(model).await.unwrap();
	let agent:Entry=serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"fixture"},"config":{"model":reference("model"),"instructions":"Original instructions"}})).unwrap();
	f.registry.register(agent).await.unwrap();
	approve(&f, "a", &reference("agent")).await;
	approve(&f, "a", &reference("model")).await;
	let package = publish(&app, &a, "agent").await;
	let original = install(&app, &a, &package, &install_input(&package)).await;
	assert_eq!(original.0, 200, "{original:?}");
	assert_eq!(
		activate(&app, &f, "a", &original.1, 1, 0, 0, true).await.0,
		200
	);
	let old_ref = EntityRef {
		id: original.1["entry"]["id"].as_str().unwrap().into(),
		version: "1.0.0".into(),
	};
	let mut authority = bundle("a", "user");
	authority["subjects"]
		[aidash::domain::qualified_agent(&f.config.node_id, &old_ref.id, &old_ref.version)] =
		json!({"kind":"agent","roles":["manager"]});
	policy(&f, "a", 1, authority.clone()).await;
	let workspace = request(
		&app,
		&a,
		"POST",
		"/api/workspaces",
		json!({"title":"Pinned","goal":"Test pinned definitions"}),
	)
	.await;
	assert_eq!(workspace.0, 200, "{workspace:?}");
	let task_path = format!(
		"/api/workspaces/{}/tasks",
		workspace.1["id"].as_str().unwrap()
	);
	let task = request(
		&app,
		&a,
		"POST",
		&task_path,
		json!({"title":"First","description":"Pinned old run"}),
	)
	.await;
	let claimed = request(
		&app,
		&a,
		"POST",
		&format!("/api/tasks/{}/claim", task.1["id"].as_str().unwrap()),
		json!({"revision":0,"agent":old_ref}),
	)
	.await;
	assert_eq!(claimed.0, 200, "{claimed:?}");
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.agent_id, old_ref.id);
	let installed_id = original.1["installation"]["id"].as_str().unwrap();
	let changed=request(&app,&a,"POST",&format!("/api/marketplace/installations/{installed_id}"),json!({"expected_revision":1,"config":{"instructions":"Replacement instructions"},"idempotency_key":Uuid::new_v4()})).await;
	assert_eq!(changed.0, 200, "{changed:?}");
	assert_eq!(
		activate(&app, &f, "a", &original.1, 2, 1, 0, true).await.0,
		200
	);
	let new_ref = EntityRef {
		id: changed.1["entry"]["id"].as_str().unwrap().into(),
		version: "1.0.0".into(),
	};
	authority["subjects"]
		[aidash::domain::qualified_agent(&f.config.node_id, &new_ref.id, &new_ref.version)] =
		json!({"kind":"agent","roles":["manager"]});
	policy(&f, "a", 2, authority).await;
	let task2 = request(
		&app,
		&a,
		"POST",
		&task_path,
		json!({"title":"Second","description":"Select active revision"}),
	)
	.await;
	let claim_path = format!("/api/tasks/{}/claim", task2.1["id"].as_str().unwrap());
	assert_eq!(
		request(
			&app,
			&a,
			"POST",
			&claim_path,
			json!({"revision":0,"agent":old_ref})
		)
		.await
		.0,
		403
	);
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let area_path = format!("/api/runs/{}/working-area", run.id);
	let available = request(&app, &a, "GET", &area_path, Value::Null).await;
	assert_eq!(
		available.0, 404,
		"the authorized Run has no working area: {available:?}"
	);
	authorization
		.set_catalog("a", &reference("model"), 1, false, "operator")
		.await
		.unwrap();
	let denied = request(&app, &a, "GET", &area_path, Value::Null).await;
	assert_eq!(
		denied.0, 403,
		"a direct capability boundary must recheck pinned approvals: {denied:?}"
	);
	assert_eq!(
		request(
			&app,
			&a,
			"POST",
			&claim_path,
			json!({"revision":0,"agent":new_ref})
		)
		.await
		.0,
		403
	);
	authorization
		.set_catalog("a", &reference("model"), 2, true, "operator")
		.await
		.unwrap();
	assert_eq!(
		request(
			&app,
			&a,
			"POST",
			&claim_path,
			json!({"revision":0,"agent":new_ref})
		)
		.await
		.0,
		200
	);
	assert_eq!(f.store.run(run.id).await.unwrap().agent_id, old_ref.id);
	assert_eq!(
		f.registry
			.get(&old_ref.id, &old_ref.version)
			.await
			.unwrap()
			.config["instructions"],
		"Original instructions"
	);
	// A fresh harness resumes from durable rows, not a current active alias.
	assert!(
		aidash::harness::Harness {
			federation: f.clone()
		}
		.worker_once()
		.await
		.unwrap()
	);
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "THINKING");
	assert_eq!(
		activate(&app, &f, "a", &original.1, 1, 2, 1, false).await.0,
		200
	);
	assert!(
		aidash::harness::Harness {
			federation: f.clone()
		}
		.worker_once()
		.await
		.unwrap()
	);
	let paused = f.store.run(run.id).await.unwrap();
	assert_eq!(paused.control, "PAUSED");
	assert_eq!(paused.agent_id, old_ref.id);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn policy_conditions_delegation_and_denies_apply_to_marketplace_boundaries(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let b = token(&f, "b", "agent").await;
	enable(&app, &f).await;
	f.registry.register(tool("policy-source")).await.unwrap();
	approve(&f, "a", &reference("policy-source")).await;
	let package = publish(&app, &a, "policy-source").await;
	let key = package["key"].as_str().unwrap();
	let path = format!("/api/marketplace/packages/{key}");
	let installation = install(&app, &a, &package, &install_input(&package))
		.await
		.1;
	let install_path = format!(
		"/api/marketplace/installations/{}",
		installation["installation"]["id"].as_str().unwrap()
	);
	let publication = json!({"source":reference("policy-source"),"package_id":"another","author":"A","idempotency_key":Uuid::new_v4()});
	let mut revision = 1;
	for action in [
		"marketplace.browse",
		"marketplace.read",
		"marketplace.install",
		"marketplace.publish",
		"marketplace.share",
		"marketplace.redistribution.manage",
		"installation.configure",
		"registry.export",
	] {
		policy(
			&f,
			"a",
			revision,
			deny(bundle("a", "user"), json!([action])),
		)
		.await;
		revision += 1;
		let result = match action {
			"marketplace.browse" => {
				let result = request(&app, &a, "GET", "/api/marketplace/packages", Value::Null).await;
				assert_eq!(result.1, json!([]));
				continue;
			},
			"marketplace.read" => request(&app, &a, "GET", &path, Value::Null).await,
			"marketplace.install" => install(&app, &a, &package, &install_input(&package)).await,
			"marketplace.share" => request(&app, &a, "PUT", &format!("{path}/audience"), json!({"expected_revision":1,"tenants":["a","b"]})).await,
			"marketplace.redistribution.manage" => request(&app, &a, "PUT", &format!("{path}/consents/b"), json!({"expected_revision":0,"tenants":["b"]})).await,
			"installation.configure" => request(&app, &a, "POST", &install_path, json!({"expected_revision":1,"config":{"endpoint":"http://localhost:9/changed"},"idempotency_key":Uuid::new_v4()})).await,
			_ => request(&app, &a, "POST", "/api/marketplace/packages", publication.clone()).await,
		};
		assert_eq!(result.0, 403, "deny {action}: {result:?}");
	}
	policy(&f, "a", revision, bundle("a", "user")).await;
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&format!("{path}/audience"),
			json!({"expected_revision":1,"tenants":["a","b"]})
		)
		.await
		.0,
		200
	);
	let mut conditional = bundle("b", "agent");
	conditional["policies"][0]["condition"] = json!({"op":"eq","left":{"source":"resource","path":"/owner_tenant"},"right":{"source":"literal","value":"a"}});
	policy(&f, "b", 1, conditional.clone()).await;
	assert_eq!(request(&app, &b, "GET", &path, Value::Null).await.0, 200);
	conditional["policies"][0]["condition"]["right"]["value"] = json!("b");
	policy(&f, "b", 2, conditional).await;
	assert_eq!(request(&app, &b, "GET", &path, Value::Null).await.0, 403);
	let mut delegated = bundle("b", "agent");
	delegated["subjects"]["viewer"]["delegated_by"] = json!("parent");
	delegated["subjects"]["parent"] = json!({"kind":"user"});
	policy(&f, "b", 3, delegated).await;
	assert_eq!(request(&app, &b, "GET", &path, Value::Null).await.0, 403);
	let mut disabled = bundle("b", "agent");
	disabled["subjects"]["viewer"]["enabled"] = json!(false);
	policy(&f, "b", 4, disabled).await;
	assert_eq!(request(&app, &b, "GET", &path, Value::Null).await.0, 403);
	let mut default_deny = bundle("b", "agent");
	default_deny["policies"] = json!([]);
	policy(&f, "b", 5, default_deny).await;
	assert_eq!(request(&app, &b, "GET", &path, Value::Null).await.0, 403);
	let mut spoofed = publication;
	spoofed["owner_tenant"] = json!("b");
	use tower::ServiceExt;
	let rejected = app
		.clone()
		.oneshot(
			axum::http::Request::post("/api/marketplace/packages")
				.header("authorization", format!("Bearer {a}"))
				.header("content-type", "application/json")
				.body(axum::body::Body::from(spoofed.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(rejected.status(), 422);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn readable_distribution_precedes_dependency_preparation_and_bindings_pin_revisions(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let b = token(&f, "b", "user").await;
	enable(&app, &f).await;
	let model: Entry = serde_json::from_value(json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"fixture"},"config":{"provider":"openrouter","model_id":"fixture/model","endpoint":"http://localhost:9","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	f.registry.register(model).await.unwrap();
	f.registry.register(tool("dependency")).await.unwrap();
	let agent: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"fixture"},"config":{"model":reference("model"),"tools":[reference("dependency")],"instructions":"Use the exact tool"}})).unwrap();
	f.registry.register(agent).await.unwrap();
	for id in ["model", "dependency", "agent"] {
		approve(&f, "a", &reference(id)).await;
	}
	approve(&f, "b", &reference("model")).await;
	let dependency = publish(&app, &a, "dependency").await;
	let agent = request(&app, &a, "POST", "/api/marketplace/packages", json!({"source":reference("agent"),"package_id":"agent","author":"A","idempotency_key":Uuid::new_v4()})).await.1;
	for package in [&dependency, &agent] {
		assert_eq!(
			request(
				&app,
				&a,
				"PUT",
				&format!(
					"/api/marketplace/packages/{}/audience",
					package["key"].as_str().unwrap()
				),
				json!({"expected_revision":1,"tenants":["a","b"]})
			)
			.await
			.0,
			200
		);
	}
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			&format!(
				"/api/marketplace/packages/{}",
				agent["key"].as_str().unwrap()
			),
			Value::Null
		)
		.await
		.0,
		200,
		"a readable dependency package must not require a prior installation"
	);
	assert_eq!(
		install(&app, &b, &agent, &install_input(&agent)).await.0,
		403
	);
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			"/api/marketplace/installations",
			Value::Null
		)
		.await
		.1,
		json!([])
	);
	let dep_local = install(&app, &b, &dependency, &install_input(&dependency)).await;
	assert_eq!(dep_local.0, 200, "{dep_local:?}");
	let root = install(&app, &b, &agent, &install_input(&agent)).await;
	assert_eq!(root.0, 200, "{root:?}");
	assert_eq!(
		root.1["entry"]["config"]["tools"][0]["id"],
		dep_local.1["entry"]["id"]
	);
	assert_eq!(activate(&app, &f, "b", &root.1, 1, 0, 0, true).await.0, 403);
	assert_eq!(
		activate(&app, &f, "b", &dep_local.1, 1, 0, 0, true).await.0,
		200
	);
	assert_eq!(activate(&app, &f, "b", &root.1, 1, 0, 0, true).await.0, 200);
	let dep_changed = request(&app, &b, "POST", &format!("/api/marketplace/installations/{}",dep_local.1["installation"]["id"].as_str().unwrap()), json!({"expected_revision":1,"config":{"endpoint":"http://localhost:9/new"},"idempotency_key":Uuid::new_v4()})).await;
	assert_eq!(dep_changed.0, 200, "{dep_changed:?}");
	for config in [
		json!({"model":reference("model")}),
		json!({"tools":[]}),
		json!({"skills":[]}),
		json!({"cluster":null}),
	] {
		let mut input = install_input(&agent);
		input["config"] = config.clone();
		assert_eq!(install(&app, &b, &agent, &input).await.0, 400);
		assert_eq!(
			request(
				&app,
				&b,
				"POST",
				&format!(
					"/api/marketplace/installations/{}",
					root.1["installation"]["id"].as_str().unwrap()
				),
				json!({"expected_revision":1,"config":config,"idempotency_key":Uuid::new_v4()})
			)
			.await
			.0,
			400
		);
	}
	let root_path = format!(
		"/api/marketplace/installations/{}",
		root.1["installation"]["id"].as_str().unwrap()
	);
	let bindings = json!([{"source":reference("dependency"),"target":{"id":dep_changed.1["entry"]["id"],"version":"1.0.0"}}]);
	let next = request(
		&app,
		&b,
		"POST",
		&root_path,
		json!({"expected_revision":1,"config":{},"bindings":bindings,"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(next.0, 200, "{next:?}");
	assert_eq!(next.1["revision"], 2);
	assert_eq!(next.1["approved"], false);
	assert_eq!(next.1["installation"]["active_revision"], 1);
	assert_eq!(
		root.1["entry"]["config"]["tools"][0]["id"],
		dep_local.1["entry"]["id"]
	);
	let wrong_kind = json!([{"source":reference("dependency"),"target":reference("model")}]);
	assert_eq!(
		request(
			&app,
			&b,
			"POST",
			&root_path,
			json!({"expected_revision":2,"config":{},"bindings":wrong_kind,"idempotency_key":Uuid::new_v4()})
		)
		.await
		.0,
		403
	);
	// Pending configuration must not replace the still-authorized active
	// dependency when the publisher withdraws distribution to this recipient.
	let mut authority = bundle("b", "user");
	authority["policies"].as_array_mut().unwrap().push(json!({"id":"deny-pending","effect":"deny","subjects":{"any":true},"actions":["installation.read"],"resources":{"kinds":["installation"],"ids":[dep_local.1["installation"]["id"]]},"condition":{"op":"eq","left":{"source":"resource","path":"/installation_revision"},"right":{"source":"literal","value":2}}}));
	policy(&f, "b", 1, authority).await;
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&format!(
				"/api/marketplace/packages/{}/audience",
				dependency["key"].as_str().unwrap()
			),
			json!({"expected_revision":2,"tenants":["a"]})
		)
		.await
		.0,
		200
	);
	let detail = request(
		&app,
		&b,
		"GET",
		&format!(
			"/api/marketplace/packages/{}",
			agent["key"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(
		detail.0, 200,
		"the active local dependency must retain manifest access: {detail:?}"
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn response_handoff_releases_locks_before_body_drain_and_rechecks_event_replay(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use axum::{body::Body, http::Request};
	use futures_util::StreamExt;
	use tower::ServiceExt;
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let b = token(&f, "b", "user").await;
	enable(&app, &f).await;
	f.registry.register(tool("handoff")).await.unwrap();
	approve(&f, "a", &reference("handoff")).await;
	let package = publish(&app, &a, "handoff").await;
	let key = package["key"].as_str().unwrap();
	let audience = format!("/api/marketplace/packages/{key}/audience");
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&audience,
			json!({"expected_revision":1,"tenants":["a","b"]})
		)
		.await
		.0,
		200
	);
	let state = request(&app, &b, "GET", "/api/state", Value::Null).await;
	assert_eq!(state.0, 200, "{state:?}");
	assert!(
		state.1["events"]
			.as_array()
			.unwrap()
			.iter()
			.any(|e| e["kind"] == "marketplace.published" && e["data"]["key"] == key)
	);
	assert!(!state.1["events"].to_string().contains("marketplace.audit"));
	let get = |path: &str| {
		Request::get(path)
			.header("authorization", format!("Bearer {b}"))
			.body(Body::empty())
			.unwrap()
	};
	// The shared SSE service must include authorized global Marketplace events,
	// even though this subject has no Workspace events. Audit events stay hidden.
	let readable = app
		.clone()
		.oneshot(get("/api/events/stream?after=0"))
		.await
		.unwrap();
	assert_eq!(readable.status(), 200);
	let mut readable = readable.into_body().into_data_stream();
	let frame = tokio::time::timeout(std::time::Duration::from_secs(3), readable.next())
		.await
		.expect("authorized Marketplace replay must emit a frame")
		.unwrap()
		.unwrap();
	let frame = String::from_utf8_lossy(&frame);
	assert!(frame.contains("marketplace.published"), "{frame}");
	assert!(frame.contains(key), "{frame}");
	assert!(!frame.contains("marketplace.audit"), "{frame}");
	drop(readable);
	// Do not poll either body until the distribution has been withdrawn.
	let response = app
		.clone()
		.oneshot(get(&format!("/api/marketplace/packages/{key}")))
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	assert_eq!(response.headers()["cache-control"], "no-store");
	let stream = app
		.clone()
		.oneshot(get("/api/events/stream?after=0"))
		.await
		.unwrap();
	assert_eq!(stream.status(), 200);
	let withdrawn = tokio::time::timeout(
		std::time::Duration::from_secs(3),
		request(
			&app,
			&a,
			"PUT",
			&audience,
			json!({"expected_revision":2,"tenants":["a"]}),
		),
	)
	.await
	.expect("slow HTTP or SSE consumers must not retain database leases");
	assert_eq!(withdrawn.0, 200);
	let state = request(&app, &b, "GET", "/api/state", Value::Null).await;
	assert_eq!(state.0, 200);
	assert!(
		!state.1["events"].to_string().contains(key),
		"withdrawn package leaked through initial state"
	);

	let authorized_before_revoke = axum::body::to_bytes(response.into_body(), 2_097_152)
		.await
		.unwrap();
	assert!(String::from_utf8_lossy(&authorized_before_revoke).contains("handoff"));
	let mut frames = stream.into_body().into_data_stream();
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(650), frames.next())
			.await
			.is_err(),
		"replay admitted before withdrawal must not newly queue a protected frame"
	);
	drop(frames);
	assert_eq!(
		request(&app, &b, "GET", "/api/events", Value::Null).await.1,
		json!([])
	);
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			&format!("/api/marketplace/packages/{key}"),
			Value::Null
		)
		.await
		.0,
		403
	);
	let events = f.store.events(0, None, 1000).await.unwrap();
	let audit = events
		.iter()
		.find(|e| {
			e.kind == "marketplace.audit"
				&& e.data["outcome"] == "denied"
				&& e.data["tenant"] == "b"
		})
		.expect("resource denial audit survives the rolled back request");
	assert!(audit.data["request_id"].is_string());
	assert!(audit.data["policy_revision"].is_number());
	assert!(!audit.data.to_string().contains(&b));
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case("dependency", false)]
#[case("dependency", true)]
#[case("consent", false)]
#[case("consent", true)]
#[tokio::test]
async fn live_dependency_and_consent_leases_order_installation_with_revocation(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] boundary: &str,
	#[case] installation_wins: bool,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let b = token(&f, "b", "user").await;
	let c = token(&f, "c", "user").await;
	enable(&app, &f).await;
	f.registry.register(tool("race-root")).await.unwrap();
	f.registry.register(tool("race-dependency")).await.unwrap();
	for tenant in ["a", "c"] {
		approve(&f, tenant, &reference("race-dependency")).await;
	}
	approve(&f, "a", &reference("race-root")).await;
	let publication = request(&app, &a, "POST", "/api/marketplace/packages", json!({"source":reference("race-root"),"package_id":"root","author":"A","dependencies":if boundary=="dependency" { vec![reference("race-dependency")] } else {vec![]},"idempotency_key":Uuid::new_v4()})).await;
	assert_eq!(publication.0, 200, "{publication:?}");
	let mut package = publication.1;
	let source_key = package["key"].as_str().unwrap().to_owned();
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&format!("/api/marketplace/packages/{source_key}/audience"),
			json!({"expected_revision":1,"tenants":["a","b","c"]})
		)
		.await
		.0,
		200
	);
	let consent_path = format!("/api/marketplace/packages/{source_key}/consents/b");
	if boundary == "consent" {
		assert_eq!(
			request(
				&app,
				&a,
				"PUT",
				&consent_path,
				json!({"expected_revision":0,"tenants":["b","c"]})
			)
			.await
			.0,
			200
		);
		let imported = install(&app, &b, &package, &install_input(&package)).await;
		assert_eq!(imported.0, 200);
		assert_eq!(
			activate(&app, &f, "b", &imported.1, 1, 0, 0, true).await.0,
			200
		);
		package = request(&app, &b, "POST", "/api/marketplace/packages", json!({"source":{"id":imported.1["entry"]["id"],"version":"1.0.0"},"package_id":"child","author":"B","idempotency_key":Uuid::new_v4()})).await.1;
		assert_eq!(
			request(
				&app,
				&b,
				"PUT",
				&format!(
					"/api/marketplace/packages/{}/audience",
					package["key"].as_str().unwrap()
				),
				json!({"expected_revision":1,"tenants":["b","c"]})
			)
			.await
			.0,
			200
		);
	}
	let mut barrier = f.store.pool.begin().await.unwrap();
	if installation_wins || boundary == "consent" {
		let lock = if installation_wins {
			"pg_advisory_xact_lock(71003201)"
		} else {
			"pg_advisory_xact_lock(74003201)"
		};
		sqlx::query(
			&Query::select()
				.expr(Expr::cust(lock))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *barrier)
		.await
		.unwrap();
	}
	if !installation_wins {
		if boundary == "dependency" {
			sqlx::query(
				&Query::update()
					.table(Alias::new("authorization_catalog"))
					.value(Alias::new("enabled"), false)
					.and_where(Expr::cust("tenant='c' AND entry_id='race-dependency'"))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *barrier)
			.await
			.unwrap();
		} else {
			let consent_key = aidash::registry::digest(&json!([source_key, "b"]))
				.trim_start_matches("sha256:")
				.to_owned();
			sqlx::query(
				&Query::update()
					.table(Alias::new("marketplace_consents"))
					.value(Alias::new("document"), Expr::cust("$2"))
					.and_where(Expr::cust("key=$1"))
					.to_string(PostgresQueryBuilder),
			)
			.bind(consent_key)
			.bind(json!({"revision":2,"tenants":[]}))
			.execute(&mut *barrier)
			.await
			.unwrap();
		}
	}
	let (app2, c2, package2) = (app.clone(), c.clone(), package.clone());
	let installing =
		tokio::spawn(
			async move { install(&app2, &c2, &package2, &install_input(&package2)).await },
		);
	wait_for_lock(
		&f,
		&schema,
		if installation_wins {
			"%71003201%"
		} else if boundary == "consent" {
			"%74003201%"
		} else {
			"%authorization_catalog%"
		},
	)
	.await;
	let revoking = if installation_wins {
		let (app2, a2, f2) = (app.clone(), a.clone(), f.clone());
		let dependency = boundary == "dependency";
		let task = tokio::spawn(async move {
			if dependency {
				Authorization {
					pool: f2.store.pool.clone(),
				}
				.set_catalog("c", &reference("race-dependency"), 1, false, "operator")
				.await
				.unwrap();
			} else {
				assert_eq!(
					request(
						&app2,
						&a2,
						"PUT",
						&consent_path,
						json!({"expected_revision":1,"tenants":[]})
					)
					.await
					.0,
					200
				);
			}
		});
		wait_for_lock(
			&f,
			&schema,
			if dependency {
				"%authorization_catalog%"
			} else {
				"%74003201%"
			},
		)
		.await;
		Some(task)
	} else {
		None
	};
	barrier.commit().await.unwrap();
	let result = installing.await.unwrap();
	if let Some(task) = revoking {
		task.await.unwrap();
	}
	assert_eq!(
		result.0,
		if installation_wins { 200 } else { 403 },
		"{boundary}: {result:?}"
	);
	let successes = f
		.store
		.events(0, None, 1000)
		.await
		.unwrap()
		.into_iter()
		.filter(|e| e.kind == "marketplace.installed" && e.data["tenant"] == "c")
		.count();
	assert_eq!(successes, usize::from(installation_wins));
	assert_eq!(
		install(&app, &c, &package, &install_input(&package))
			.await
			.0,
		403
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn immutable_versions_replays_and_recovery_keep_their_authority_boundaries(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use migration::MigratorTrait;
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let b = token(&f, "b", "user").await;
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	migration::Migrator::down(&db, Some(1)).await.unwrap();
	migration::Migrator::up(&db, None).await.unwrap();
	enable(&app, &f).await;
	f.registry.register(tool("immutable-source")).await.unwrap();
	approve(&f, "a", &reference("immutable-source")).await;
	f.registry.register(tool("published-later")).await.unwrap();
	approve(&f, "a", &reference("published-later")).await;
	approve(&f, "b", &reference("published-later")).await;
	let input = json!({"source":reference("immutable-source"),"package_id":"immutable","author":"A","dependencies":[reference("published-later")],"idempotency_key":Uuid::new_v4()});
	let published = request(&app, &a, "POST", "/api/marketplace/packages", input.clone()).await;
	assert_eq!(published.0, 200, "{published:?}");
	let renewed = Authorization {
		pool: f.store.pool.clone(),
	}
	.issue_credential("a", "viewer", 3600, "operator")
	.await
	.unwrap()
	.token;
	assert_eq!(
		request(
			&api::router(f.clone()),
			&renewed,
			"POST",
			"/api/marketplace/packages",
			input.clone()
		)
		.await,
		published
	);
	assert_eq!(
		f.store
			.events(0, None, 1000)
			.await
			.unwrap()
			.iter()
			.filter(|e| e.kind == "marketplace.published")
			.count(),
		1
	);
	// Publishing an already-readable dependency later must not reinterpret an
	// earlier idempotent result's immutable provenance/graph.
	publish(&app, &a, "published-later").await;
	assert_eq!(
		request(
			&app,
			&renewed,
			"POST",
			"/api/marketplace/packages",
			input.clone()
		)
		.await,
		published
	);
	let key = published.1["key"].as_str().unwrap();
	let audience = format!("/api/marketplace/packages/{key}/audience");
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&audience,
			json!({"expected_revision":1,"tenants":["a","b"]})
		)
		.await
		.0,
		200
	);
	let mut second = tool("immutable-source");
	second.version = "2.0.0".into();
	f.registry.register(second).await.unwrap();
	let second_ref = EntityRef {
		id: "immutable-source".into(),
		version: "2.0.0".into(),
	};
	approve(&f, "a", &second_ref).await;
	let second = request(&app,&a,"POST","/api/marketplace/packages",json!({"source":second_ref,"package_id":"immutable","author":"A","idempotency_key":Uuid::new_v4()})).await;
	assert_eq!(second.0, 200, "{second:?}");
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			&format!(
				"/api/marketplace/packages/{}",
				second.1["key"].as_str().unwrap()
			),
			Value::Null
		)
		.await
		.0,
		403,
		"audiences never carry to a new version"
	);
	let installed = install(&app, &a, &published.1, &install_input(&published.1)).await;
	assert_eq!(installed.0, 200);
	let protected_id = installed.1["installation"]["id"].as_str().unwrap();
	let replay_id = Uuid::new_v4();
	let mut repeated = install_input(&published.1);
	repeated["idempotency_key"] = json!(replay_id);
	assert_eq!(install(&app, &a, &published.1, &repeated).await.0, 200);
	let mut permission = bundle("a", "user");
	permission["policies"].as_array_mut().unwrap().push(json!({"id":"old-result","effect":"deny","subjects":{"any":true},"actions":["installation.read"],"resources":{"kinds":["installation"],"ids":[protected_id]}}));
	policy(&f, "a", 1, permission).await;
	let mut changed_target = install_input(&second.1);
	changed_target["idempotency_key"] = json!(replay_id);
	assert_eq!(
		install(&app, &a, &second.1, &changed_target).await.0,
		403,
		"a changed target must not probe an old now-hidden idempotency result"
	);
	policy(&f, "a", 2, bundle("a", "user")).await;
	let mut changed = input.clone();
	changed["author"] = json!("replacement");
	changed["idempotency_key"] = json!(Uuid::new_v4());
	assert_eq!(
		request(&app, &a, "POST", "/api/marketplace/packages", changed)
			.await
			.0,
		409
	);
	assert_eq!(
		request(
			&app,
			&a,
			"PUT",
			&audience,
			json!({"expected_revision":2,"tenants":["b"]})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&app, &a, "POST", "/api/marketplace/packages", input)
			.await
			.0,
		403,
		"publication replay cannot recover withdrawn distribution access"
	);
	assert!(
		migration::Migrator::down(&db, Some(1)).await.is_err(),
		"downgrade must preserve populated tenant projections"
	);
	// Even trusted SQL callers cannot overwrite immutable published bytes.
	let update = Query::update()
		.table(Alias::new("marketplace_versions"))
		.value(
			Alias::new("document"),
			Expr::cust("jsonb_set(document,'{manifest_source}','\"corrupted\"'::jsonb)"),
		)
		.and_where(Expr::cust("key=$1"))
		.to_string(PostgresQueryBuilder);
	assert!(
		sqlx::query(&update)
			.bind(key)
			.execute(&f.store.pool)
			.await
			.is_err()
	);
	// Fault injection only: SeaQuery has no ALTER TRIGGER API. A corrupted
	// backing store must still fail digest verification before sending bytes.
	sqlx::query("ALTER TABLE marketplace_versions DISABLE TRIGGER marketplace_immutable")
		.execute(&f.store.pool)
		.await
		.unwrap();
	sqlx::query(&update)
		.bind(key)
		.execute(&f.store.pool)
		.await
		.unwrap();
	sqlx::query("ALTER TABLE marketplace_versions ENABLE TRIGGER marketplace_immutable")
		.execute(&f.store.pool)
		.await
		.unwrap();
	assert_eq!(
		request(
			&app,
			&b,
			"GET",
			&format!("/api/marketplace/packages/{key}"),
			Value::Null
		)
		.await
		.0,
		409
	);
	assert_eq!(
		install(&app, &b, &published.1, &install_input(&published.1))
			.await
			.0,
		409
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn marketplace_event_polling_bounds_candidates_and_advances_past_denials(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	f.registry.register(tool("events")).await.unwrap();
	approve(&f, "a", &reference("events")).await;
	let package = publish(&app, &a, "events").await;
	let installed = install(&app, &a, &package, &install_input(&package)).await;
	assert_eq!(installed.0, 200, "{installed:?}");
	let after = f
		.store
		.events(0, None, 1000)
		.await
		.unwrap()
		.last()
		.unwrap()
		.sequence;
	let identity = match (Authorization {
		pool: f.store.pool.clone(),
	})
	.authenticate(&a)
	.await
	.unwrap()
	{
		aidash::authorization::identity::Actor::Subject(identity) => identity,
		_ => unreachable!(),
	};
	let reader = aidash::authorization::workspace::Workspaces {
		store: f.store.clone(),
		identity,
	};
	let mut tx = f.store.pool.begin().await.unwrap();
	for _ in 0..512 {
		f.store
			.event(
				&mut tx,
				None,
				"marketplace.audit",
				json!({"tenant":"a","installation":installed.1["installation"]["id"]}),
			)
			.await
			.unwrap();
		f.store
			.event(
				&mut tx,
				None,
				"marketplace.installed",
				json!({"tenant":"b","installation":"foreign"}),
			)
			.await
			.unwrap();
	}
	let mut denied_cursor = after;
	for i in 0..4097 {
		let event = f
			.store
			.event(
				&mut tx,
				None,
				"marketplace.installed",
				json!({"tenant":"a","installation":"removed-installation","revision":1}),
			)
			.await
			.unwrap();
		if i == 4095 {
			denied_cursor = event.sequence;
		}
	}
	tx.commit().await.unwrap();
	// A state snapshot must release its authority lease after bounded work,
	// even when older visible events exist behind thousands of denials.
	let state = request(&app, &a, "GET", "/api/state", Value::Null).await;
	assert_eq!(state.0, 200, "{state:?}");
	assert_eq!(state.1["events"], json!([]));
	let mut tx = f.store.pool.begin().await.unwrap();
	let visible = f
		.store
		.event(
			&mut tx,
			None,
			"marketplace.installed",
			json!({"tenant":"a","installation":installed.1["installation"]["id"],"revision":1}),
		)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let (events, cursor) = reader.poll_events(after, None, 1000).await.unwrap();
	assert!(
		events.is_empty(),
		"one poll must stop before the distant visible event"
	);
	assert_eq!(
		cursor, denied_cursor,
		"audits and foreign tenants must not consume the candidate budget"
	);
	let (events, cursor) = reader.poll_events(cursor, None, 1000).await.unwrap();
	assert_eq!(
		events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
		vec![visible.sequence]
	);
	assert_eq!(cursor, visible.sequence);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_inspection_obeys_the_marketplace_compatibility_gate(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	use tower::ServiceExt;
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	let model: Entry = serde_json::from_value(json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"fixture"},"config":{"provider":"openrouter","model_id":"fixture/model","endpoint":"http://localhost:9","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	let agent: Entry = serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"fixture"},"config":{"model":reference("model"),"instructions":"Installed executor"}})).unwrap();
	for entry in [model, agent] {
		let reference = EntityRef {
			id: entry.id.clone(),
			version: entry.version.clone(),
		};
		f.registry.register(entry).await.unwrap();
		approve(&f, "a", &reference).await;
	}
	let package = publish(&app, &a, "agent").await;
	let installed = install(&app, &a, &package, &install_input(&package)).await;
	assert_eq!(installed.0, 200, "{installed:?}");
	assert_eq!(
		activate(&app, &f, "a", &installed.1, 1, 0, 0, true).await.0,
		200
	);
	let entry = EntityRef {
		id: installed.1["entry"]["id"].as_str().unwrap().into(),
		version: "1.0.0".into(),
	};
	let mut authority = bundle("a", "user");
	authority["subjects"]
		[aidash::domain::qualified_agent(&f.config.node_id, &entry.id, &entry.version)] =
		json!({"kind":"agent","roles":["manager"]});
	policy(&f, "a", 1, authority).await;
	let authorization = Authorization {
		pool: f.store.pool.clone(),
	};
	let credential = authorization
		.issue_credential("a", "viewer", 3600, "operator")
		.await
		.unwrap();
	let peer = "aidash://marketplace-inspector";
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("peers"))
			.columns(
				[
					"node_id",
					"endpoint",
					"credential_env",
					"protocol_version",
					"enabled",
				]
				.map(Alias::new),
			)
			.values_panic([
				Expr::value(peer),
				Expr::value("http://127.0.0.1:9"),
				Expr::value("AIDASH_SECRET_TEST_PEER"),
				Expr::value(aidash::config::PROTOCOL_VERSION),
				Expr::value(true),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let mapped = request(&app,&f.config.api_token,"POST","/api/authorization/a/peer-mappings",json!({"source_node":peer,"source_tenant":"source","source_subject":"viewer","credential_id":credential.credential.id,"expected_revision":0,"enabled":true})).await;
	assert_eq!(mapped.0, 200, "{mapped:?}");
	let inspect = || {
		axum::http::Request::post("/federation/v0.1/scoped/execution/inspect")
			.header(
				"authorization",
				format!(
					"Bearer {}",
					std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()
				),
			)
			.header("x-aidash-node", peer)
			.header("x-aidash-protocol", aidash::config::PROTOCOL_VERSION)
			.header("content-type", "application/json")
			.body(axum::body::Body::from(
				json!({"tenant":"source","subject":"viewer","agent":entry,"requirements":{}})
					.to_string(),
			))
			.unwrap()
	};
	assert_eq!(app.clone().oneshot(inspect()).await.unwrap().status(), 200);
	let disabled = request(
		&app,
		&f.config.api_token,
		"PUT",
		"/api/marketplace/compatibility",
		json!({"enabled":false,"expected_revision":2,"compatible_instances_confirmed":true}),
	)
	.await;
	assert_eq!(disabled.0, 200, "{disabled:?}");
	assert_eq!(
		app.clone().oneshot(inspect()).await.unwrap().status(),
		403,
		"an installed definition must not be disclosed while the gate is off"
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case("install", "logout", false)]
#[case("install", "logout", true)]
#[case("compatibility", "logout", false)]
#[case("compatibility", "logout", true)]
#[case("activation", "logout", false)]
#[case("activation", "logout", true)]
#[case("adoption", "logout", false)]
#[case("adoption", "logout", true)]
#[case("compatibility", "operator_grant", false)]
#[case("compatibility", "operator_grant", true)]
#[case("compatibility", "expiry", false)]
#[tokio::test]
async fn browser_authority_is_ordered_with_pending_marketplace_mutations(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] operation: &str,
	#[case] revocation: &str,
	#[case] mutation_wins: bool,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	use sha2::{Digest, Sha256};
	use tower::ServiceExt;
	let _fixture = FIXTURE_LOCK.lock().await;
	let (mut f, url, schema) = common::setup(&environment).await;
	f.config.oidc = Some(aidash::config::OidcConfig {
		issuer: "https://accounts.google.com".into(),
		client_id: "fixture".into(),
		client_secret: "fixture".into(),
		public_origin: "http://127.0.0.1:8080".into(),
		keycloak_admin_url: String::new(),
		status_client_id: String::new(),
		status_client_secret: String::new(),
		session_absolute_seconds: 3600,
		session_idle_seconds: 1800,
	});
	let app = api::router(f.clone()).layer(axum::Extension(axum::extract::ConnectInfo(
		"127.0.0.1:12345".parse::<std::net::SocketAddr>().unwrap(),
	)));
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	f.registry.register(tool("browser-source")).await.unwrap();
	approve(&f, "a", &reference("browser-source")).await;
	let package = publish(&app, &a, "browser-source").await;
	let identity = Uuid::new_v4();
	let session = Uuid::new_v4();
	let mapping = Uuid::new_v4();
	let issued = Authorization {
		pool: f.store.pool.clone(),
	}
	.issue_credential("a", "viewer", 3600, "operator")
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("dashboard_identities"))
			.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
			.values_panic([
				Expr::cust("$1"),
				Expr::value("https://accounts.google.com"),
				Expr::value("fixture-user"),
				Expr::cust("clock_timestamp()"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(identity)
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("dashboard_sessions"))
			.columns(
				[
					"id",
					"identity_id",
					"token_hash",
					"csrf_hash",
					"expires_at",
					"created_at",
					"last_activity_at",
				]
				.map(Alias::new),
			)
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::cust("$3"),
				Expr::cust("$4"),
				Expr::cust("clock_timestamp()+interval '1 hour'"),
				Expr::cust("clock_timestamp()"),
				Expr::cust("clock_timestamp()"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(session)
	.bind(identity)
	.bind(Sha256::digest(b"marketplace-test-session").to_vec())
	.bind(Sha256::digest(b"marketplace-test-csrf").to_vec())
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("dashboard_mappings"))
			.columns(["id", "identity_id", "tenant", "subject", "credential_id"].map(Alias::new))
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::value("a"),
				Expr::value("viewer"),
				Expr::cust("$3"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(mapping)
	.bind(identity)
	.bind(issued.credential.id)
	.execute(&f.store.pool)
	.await
	.unwrap();

	let operator = operation != "install";
	if operator {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("dashboard_operator_grants"))
				.columns(["identity_id", "enabled", "revision"].map(Alias::new))
				.values_panic([Expr::cust("$1"), Expr::value(true), Expr::value(1)])
				.to_string(PostgresQueryBuilder),
		)
		.bind(identity)
		.execute(&f.store.pool)
		.await
		.unwrap();
	}
	let (method, path, input, event_kind) = match operation {
		"install" => (
			"POST",
			format!(
				"/api/marketplace/packages/{}/install",
				package["key"].as_str().unwrap()
			),
			install_input(&package),
			"marketplace.installed",
		),
		"compatibility" => (
			"PUT",
			"/api/marketplace/compatibility".into(),
			json!({"enabled":false,"expected_revision":2,"compatible_instances_confirmed":true}),
			"marketplace.compatibility_changed",
		),
		"activation" => {
			let installed = install(&app, &a, &package, &install_input(&package)).await;
			assert_eq!(installed.0, 200, "{installed:?}");
			(
				"POST",
				format!(
					"/api/marketplace/installations/{}/activation",
					installed.1["installation"]["id"].as_str().unwrap()
				),
				json!({"tenant":"a","revision":1,"expected_activation_revision":0,"expected_catalog_revision":0,"enabled":true}),
				"marketplace.activation_changed",
			)
		}
		"adoption" => {
			f.registry.register(tool("browser-legacy")).await.unwrap();
			f.registry
				.publish(
					&f.store.pool,
					aidash::registry::Package {
						entity: tool("browser-legacy"),
						author: "legacy".into(),
						permissions: vec![],
						dependencies: vec![],
					},
				)
				.await
				.unwrap();
			(
				"POST",
				"/api/marketplace/adoptions".into(),
				json!({"tenant":"a","source":reference("browser-legacy"),"idempotency_key":Uuid::new_v4()}),
				"marketplace.installed",
			)
		}
		_ => unreachable!(),
	};
	let before = f
		.store
		.events(0, None, 1000)
		.await
		.unwrap()
		.iter()
		.filter(|e| e.kind == event_kind)
		.count();
	if revocation == "expiry" {
		sqlx::query(
			&Query::update()
				.table(Alias::new("dashboard_sessions"))
				.value(
					Alias::new("expires_at"),
					Expr::cust("clock_timestamp()+interval '2 seconds'"),
				)
				.and_where(Expr::cust("id=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(session)
		.execute(&f.store.pool)
		.await
		.unwrap();
	}
	let grant_revoke = Query::update()
		.table(Alias::new("dashboard_operator_grants"))
		.value(Alias::new("enabled"), false)
		.value(Alias::new("revision"), Expr::cust("revision+1"))
		.and_where(Expr::cust("identity_id=$1"))
		.to_string(PostgresQueryBuilder);
	let cookie_request = |method: &str, path: &str, value: Value| {
		axum::http::Request::builder()
			.method(method)
			.uri(path)
			.header("cookie", "aidash-session=marketplace-test-session")
			.header("origin", "http://127.0.0.1:8080")
			.header("x-aidash-csrf", "marketplace-test-csrf")
			.header(
				"x-aidash-context",
				if operator {
					"operator".into()
				} else {
					format!("mapping:{mapping}")
				},
			)
			.header("content-type", "application/json")
			.body(axum::body::Body::from(value.to_string()))
			.unwrap()
	};
	let mut barrier = f.store.pool.begin().await.unwrap();
	let wait_at_commit = mutation_wins || revocation == "expiry";
	if wait_at_commit {
		sqlx::query(
			&Query::select()
				.expr(Expr::cust("pg_advisory_xact_lock(71003201)"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *barrier)
		.await
		.unwrap();
	} else if revocation == "operator_grant" {
		sqlx::query(&grant_revoke)
			.bind(identity)
			.execute(&mut *barrier)
			.await
			.unwrap();
	} else {
		sqlx::query(
			&Query::update()
				.table(Alias::new("dashboard_sessions"))
				.value(Alias::new("revoked_at"), Expr::cust("clock_timestamp()"))
				.and_where(Expr::cust("id=$1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(session)
		.execute(&mut *barrier)
		.await
		.unwrap();
	}
	let request = cookie_request(method, &path, input);
	let app2 = app.clone();
	let installing = tokio::spawn(async move { app2.oneshot(request).await.unwrap() });
	wait_for_lock(
		&f,
		&schema,
		if wait_at_commit {
			"%71003201%"
		} else if revocation == "operator_grant" {
			"%dashboard_operator_grants%"
		} else {
			"%dashboard_sessions%"
		},
	)
	.await;
	let revoking = if mutation_wins {
		let task = if revocation == "operator_grant" {
			let pool = f.store.pool.clone();
			let sql = grant_revoke.clone();
			tokio::spawn(async move {
				sqlx::query(&sql)
					.bind(identity)
					.execute(&pool)
					.await
					.unwrap();
				200u16
			})
		} else {
			let request = cookie_request("POST", "/auth/logout", json!({}));
			let app2 = app.clone();
			tokio::spawn(async move { app2.oneshot(request).await.unwrap().status().as_u16() })
		};
		wait_for_lock(
			&f,
			&schema,
			if revocation == "operator_grant" {
				"%dashboard_operator_grants%"
			} else {
				"%dashboard_sessions%"
			},
		)
		.await;
		Some(task)
	} else {
		None
	};
	if revocation == "expiry" {
		tokio::time::sleep(std::time::Duration::from_secs(3)).await;
	}
	barrier.commit().await.unwrap();
	assert_eq!(
		installing.await.unwrap().status().as_u16(),
		if mutation_wins {
			200
		} else if revocation == "operator_grant" {
			403
		} else {
			401
		}
	);
	if let Some(task) = revoking {
		assert_eq!(
			task.await.unwrap(),
			if revocation == "operator_grant" {
				200
			} else {
				204
			}
		);
	}
	assert_eq!(
		f.store
			.events(0, None, 1000)
			.await
			.unwrap()
			.iter()
			.filter(|e| e.kind == event_kind)
			.count(),
		before + usize::from(mutation_wins)
	);
	// The separate durable credential stays valid after logout; the browser
	// session does not become a lifetime requirement for an already admitted Run.
	assert!(
		Authorization {
			pool: f.store.pool.clone()
		}
		.authenticate(&issued.token)
		.await
		.is_ok()
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn global_event_cursors_bound_workspace_and_marketplace_candidates(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use axum::{body::Body, http::Request};
	use futures_util::StreamExt;
	use tower::ServiceExt;
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	let workspace = request(
		&app,
		&a,
		"POST",
		"/api/workspaces",
		json!({"title":"Cursor", "goal":"Bound replay"}),
	)
	.await;
	assert_eq!(workspace.0, 200);
	let ws = workspace.1["id"].as_str().unwrap().parse().unwrap();
	let mut after = 0;
	for i in 0..501 {
		after = f
			.store
			.emit(Some(ws), "workspace.updated", json!({"id":ws,"number":i}))
			.await
			.unwrap()
			.sequence;
	}
	enable(&app, &f).await;
	f.registry.register(tool("cursor-source")).await.unwrap();
	approve(&f, "a", &reference("cursor-source")).await;
	let package = publish(&app, &a, "cursor-source").await;
	let newest = f
		.store
		.emit(Some(ws), "workspace.updated", json!({"id":ws,"number":502}))
		.await
		.unwrap();
	let result = tokio::time::timeout(
		std::time::Duration::from_secs(5),
		request(
			&app,
			&a,
			"GET",
			&format!("/api/events?after={after}&limit=1000"),
			Value::Null,
		),
	)
	.await
	.expect("global polling must advance past a full page of old workspace events");
	assert_eq!(result.0, 200);
	let events = result.1.as_array().unwrap();
	assert!(
		events
			.iter()
			.all(|e| e["sequence"].as_i64().unwrap() > after)
	);
	assert!(events.iter().any(|e| e["data"]["key"] == package["key"]));
	assert!(events.iter().any(|e| e["sequence"] == newest.sequence));
	let response = app
		.clone()
		.oneshot(
			Request::get(format!("/api/events/stream?after={after}"))
				.header("authorization", format!("Bearer {a}"))
				.body(Body::empty())
				.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	let mut frames = response.into_body().into_data_stream();
	let frame = tokio::time::timeout(std::time::Duration::from_secs(5), frames.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	let frame = String::from_utf8_lossy(&frame);
	assert!(
		frame.contains("marketplace.published"),
		"old workspace frames must not replay: {frame}"
	);
	drop(frames);
	let state = request(&app, &a, "GET", "/api/state", Value::Null).await;
	assert_eq!(state.0, 200);
	assert!(
		state.1["events"]
			.as_array()
			.unwrap()
			.iter()
			.any(|e| e["data"]["key"] == package["key"])
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn transitive_bindings_cannot_claim_to_rewrite_immutable_dependencies(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	let model:Entry=serde_json::from_value(json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"fixture"},"config":{"provider":"openrouter","model_id":"fixture/model","endpoint":"http://localhost:9","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	f.registry.register(model).await.unwrap();
	let agent:Entry=serde_json::from_value(json!({"id":"nested","version":"1.0.0","kind":"agent","name":{"en":"Nested"},"description":{"en":"fixture"},"config":{"model":reference("model"),"instructions":"Nested agent"}})).unwrap();
	f.registry.register(agent.clone()).await.unwrap();
	let mut replacement = agent.clone();
	replacement.id = "replacement".into();
	f.registry.register(replacement).await.unwrap();
	let mut bridge = tool("bridge");
	bridge.config =
		json!({"transport":"agent","node_id":f.config.node_id,"agent":reference("nested")});
	f.registry.register(bridge).await.unwrap();
	let mut root = agent;
	root.id = "root".into();
	root.config["tools"] = json!([reference("bridge")]);
	f.registry.register(root).await.unwrap();
	for id in ["model", "nested", "replacement", "bridge", "root"] {
		approve(&f, "a", &reference(id)).await;
	}
	let package = publish(&app, &a, "root").await;
	let mut input = install_input(&package);
	input["bindings"] = json!([{"source":reference("nested"),"target":reference("replacement")}]);
	assert_eq!(install(&app, &a, &package, &input).await.0, 403);
	let installed = install(&app, &a, &package, &install_input(&package)).await;
	assert_eq!(installed.0, 200, "{installed:?}");
	assert_eq!(request(&app, &a, "POST", &format!("/api/marketplace/installations/{}",installed.1["installation"]["id"].as_str().unwrap()),
		json!({"expected_revision":1,"config":{},"bindings":input["bindings"],"idempotency_key":Uuid::new_v4()})).await.0, 403);
	let bridge_package = request(&app, &a, "POST", "/api/marketplace/packages", json!({"source":reference("bridge"),"package_id":"bridge","author":"A","idempotency_key":Uuid::new_v4()})).await.1;
	let mut bridge_input = install_input(&bridge_package);
	bridge_input["bindings"] = input["bindings"].clone();
	let bound = install(&app, &a, &bridge_package, &bridge_input).await;
	assert_eq!(bound.0, 200, "direct bindings must still work: {bound:?}");
	assert_eq!(
		bound.1["entry"]["config"]["agent"],
		json!(reference("replacement"))
	);
	for config in [
		json!({"agent":reference("replacement")}),
		json!({"node_id":f.config.node_id}),
		json!({"transport":"agent"}),
	] {
		bridge_input["config"] = config;
		bridge_input["idempotency_key"] = json!(Uuid::new_v4());
		assert_eq!(
			install(&app, &a, &bridge_package, &bridge_input).await.0,
			400
		);
	}
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn installation_rejects_missing_private_knowledge(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	use sha2::{Digest, Sha256};
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	let model: Entry = serde_json::from_value(json!({"id":"private-model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"fixture"},"config":{"provider":"openrouter","model_id":"fixture/model","endpoint":"http://localhost:9","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	f.registry.register(model).await.unwrap();
	let documents = json!([{"name":"Note","media_type":"text/plain","text":"source"}]);
	let digest = format!("{:x}", Sha256::digest(documents.to_string().as_bytes()));
	let agent: Entry = serde_json::from_value(json!({"id":"private-agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"fixture"},"config":{"model":reference("private-model"),"instructions":"Private context","knowledge_digest":digest}})).unwrap();
	f.registry.register(agent).await.unwrap();
	for id in ["private-model", "private-agent"] {
		approve(&f, "a", &reference(id)).await;
	}
	let package = publish(&app, &a, "private-agent").await;
	let failed = install(&app, &a, &package, &install_input(&package)).await;
	assert_eq!(failed.0, 403, "{failed:?}");
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("agent_knowledge"))
			.columns(["agent_id", "agent_version", "documents"].map(Alias::new))
			.values_panic(["$1", "$2", "$3"].map(Expr::cust))
			.to_string(PostgresQueryBuilder),
	)
	.bind("private-agent")
	.bind("1.0.0")
	.bind(json!([]))
	.execute(&f.store.pool)
	.await
	.unwrap();
	let mismatched = install(&app, &a, &package, &install_input(&package)).await;
	assert_eq!(mismatched.0, 403, "{mismatched:?}");
	sqlx::query(
		&Query::update()
			.table(Alias::new("agent_knowledge"))
			.value(Alias::new("documents"), Expr::cust("$1"))
			.and_where(Expr::cust(
				"agent_id='private-agent' AND agent_version='1.0.0'",
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(documents)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let installed = install(&app, &a, &package, &install_input(&package)).await;
	assert_eq!(installed.0, 200, "{installed:?}");
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn compatibility_disable_orders_new_run_admission(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
	#[case] admission_wins: bool,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let _fixture = FIXTURE_LOCK.lock().await;
	let (f, url, schema) = common::setup(&environment).await;
	let app = api::router(f.clone());
	let a = token(&f, "a", "user").await;
	enable(&app, &f).await;
	let model:Entry=serde_json::from_value(json!({"id":"model","version":"1.0.0","kind":"model","name":{"en":"Model"},"description":{"en":"fixture"},"config":{"provider":"openrouter","model_id":"fixture/model","endpoint":"http://localhost:9","context_window":128000,"max_output_tokens":1024,"modalities":["text"],"cost":{}}})).unwrap();
	f.registry.register(model).await.unwrap();
	let agent:Entry=serde_json::from_value(json!({"id":"agent","version":"1.0.0","kind":"agent","name":{"en":"Agent"},"description":{"en":"fixture"},"config":{"model":reference("model"),"instructions":"Original instructions"}})).unwrap();
	f.registry.register(agent).await.unwrap();
	approve(&f, "a", &reference("agent")).await;
	approve(&f, "a", &reference("model")).await;
	let package = publish(&app, &a, "agent").await;
	let original = install(&app, &a, &package, &install_input(&package)).await;
	assert_eq!(original.0, 200, "{original:?}");
	assert_eq!(
		activate(&app, &f, "a", &original.1, 1, 0, 0, true).await.0,
		200
	);
	let old_ref = EntityRef {
		id: original.1["entry"]["id"].as_str().unwrap().into(),
		version: "1.0.0".into(),
	};
	let exact_path = format!("/api/registry/{}/{}", old_ref.id, old_ref.version);
	assert_eq!(
		request(&app, &a, "GET", &exact_path, Value::Null).await.0,
		200
	);
	let mut authority = bundle("a", "user");
	authority["subjects"]
		[aidash::domain::qualified_agent(&f.config.node_id, &old_ref.id, &old_ref.version)] =
		json!({"kind":"agent","roles":["manager"]});
	policy(&f, "a", 1, authority.clone()).await;
	let workspace = request(
		&app,
		&a,
		"POST",
		"/api/workspaces",
		json!({"title":"Pinned","goal":"Test pinned definitions"}),
	)
	.await;
	assert_eq!(workspace.0, 200, "{workspace:?}");
	let task_path = format!(
		"/api/workspaces/{}/tasks",
		workspace.1["id"].as_str().unwrap()
	);
	let task = request(
		&app,
		&a,
		"POST",
		&task_path,
		json!({"title":"First","description":"Pinned old run"}),
	)
	.await;

	let mut barrier = f.store.pool.begin().await.unwrap();
	let lock = if admission_wins {
		"pg_advisory_xact_lock(71003201)"
	} else {
		"pg_advisory_xact_lock(74003201)"
	};
	sqlx::query(
		&Query::select()
			.expr(Expr::cust(lock))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *barrier)
	.await
	.unwrap();
	if !admission_wins {
		sqlx::query(
			&Query::update()
				.table(Alias::new("marketplace_gate"))
				.value(Alias::new("document"), Expr::cust("$1"))
				.and_where(Expr::col(Alias::new("key")).eq("v1"))
				.to_string(PostgresQueryBuilder),
		)
		.bind(json!({"enabled":false,"revision":3,"contract":1}))
		.execute(&mut *barrier)
		.await
		.unwrap();
	}
	let app2 = app.clone();
	let a2 = a.clone();
	let claim_path = format!("/api/tasks/{}/claim", task.1["id"].as_str().unwrap());
	let claim = tokio::spawn(async move {
		request(
			&app2,
			&a2,
			"POST",
			&claim_path,
			json!({"revision":0,"agent":old_ref}),
		)
		.await
	});
	wait_for_lock(
		&f,
		&schema,
		if admission_wins {
			"%71003201%"
		} else {
			"%74003201%"
		},
	)
	.await;
	let disable = if admission_wins {
		let app2 = app.clone();
		let operator = f.config.api_token.clone();
		let disable = tokio::spawn(async move {
			request(
				&app2,
				&operator,
				"PUT",
				"/api/marketplace/compatibility",
				json!({"enabled":false,"expected_revision":2,"compatible_instances_confirmed":true}),
			)
			.await
		});
		wait_for_lock(&f, &schema, "%74003201%").await;
		Some(disable)
	} else {
		None
	};
	barrier.commit().await.unwrap();
	let result = tokio::time::timeout(std::time::Duration::from_secs(5), claim)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(
		result.0,
		if admission_wins { 200 } else { 403 },
		"{result:?}"
	);
	if let Some(disable) = disable {
		assert_eq!(
			tokio::time::timeout(std::time::Duration::from_secs(5), disable)
				.await
				.unwrap()
				.unwrap()
				.0,
			200
		);
	}
	assert_eq!(
		f.store.runs().await.unwrap().len(),
		usize::from(admission_wins)
	);
	assert_eq!(
		request(&app, &a, "GET", &exact_path, Value::Null).await.0,
		403
	);
	common::cleanup(f, &url, &schema).await;
}
