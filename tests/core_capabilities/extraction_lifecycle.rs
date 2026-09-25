use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

struct ExtractionFixture {
	c: CoreFixture,
	path: String,
	operation: Uuid,
	expected: &'static str,
	expected_runner: &'static str,
}

async fn runner(
	c: &CoreFixture,
	method: reqwest::Method,
	path: &str,
	body: Option<Value>,
) -> Value {
	let profile = c.f.store.capabilities.0.runner.as_ref().unwrap();
	let client = reqwest::Client::builder()
		.no_proxy()
		.timeout(std::time::Duration::from_secs(60))
		.build()
		.unwrap();
	let mut request = client
		.request(
			method,
			format!("{}{path}", profile.endpoint.trim_end_matches('/')),
		)
		.bearer_auth(std::env::var(&profile.token_env).unwrap());
	if let Some(body) = body {
		request = request.json(&body);
	}
	let reply = request.send().await.unwrap();
	if reply.status() == reqwest::StatusCode::NOT_FOUND {
		return json!({"status":"absent"});
	}
	reply.error_for_status().unwrap().json().await.unwrap()
}

#[rstest::fixture]
fn extraction_lifecycle_fixture(
	#[default("failed")] case: &'static str,
	#[future] runtime_fixture: CoreFixture,
) -> impl std::future::Future<Output = ExtractionFixture> {
	let runtime_fixture = Box::pin(runtime_fixture);
	async move {
		let mut c = runtime_fixture.await;
		let bytes = b"original bytes stay available";
		let (status, upload) = request(&c.app, &c.token, "POST", "/api/references/uploads", json!({"idempotency_key":Uuid::new_v4(),"name":"original.txt","media_type":"text/plain","size":bytes.len(),"digest":aidash::capabilities::objects::digest(bytes)})).await;
		assert_eq!(status, 200, "{upload}");
		let id: Uuid = serde_json::from_value(upload["reference_id"].clone()).unwrap();
		let path = format!("/api/references/{id}");
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("{path}/chunks"),
			json!({"offset":0,"data":STANDARD.encode(bytes)}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("{path}/commit"),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{result}");
		let mut data: Value = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("data"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_one(&c.f.store.pool)
		.await
		.unwrap();
		let operation: Uuid = serde_json::from_value(data["operation_id"].clone()).unwrap();
		let original = data["original"].clone();
		let digest = aidash::registry::digest(&json!(["extract/1", id, original]));
		let disabled = case.starts_with("disabled");
		let mut expected_runner = "absent";
		if case != "disabled_undispatched" {
			let health = runner(&c, reqwest::Method::GET, "/v1/health", None).await;
			data["instance"] = health["instance"].clone();
			data["dispatch_pending"] = json!(case == "disabled_intent");
			if case != "disabled_intent" {
				let staged = matches!(case, "cancelled" | "disabled_staged");
				let files = if staged {
					json!([{"file_id":original["file_id"],"path":"original","scope":"references","size":original["size"],"digest":original["digest"]}])
				} else {
					json!([])
				};
				let code = if case == "failed" {
					"exit 7"
				} else {
					"sleep 30; printf must-not-complete"
				};
				// Inject a real runner failure or leave its real input staging open.
				// The database models the already committed extraction intent.
				let observed = runner(&c, reqwest::Method::POST, "/v1/operations", Some(json!({"operation_id":operation,"area_id":id,"epoch":1,"digest":digest,"kind":"shell","code":code,"seconds":40,"files":files}))).await;
				if staged {
					assert_eq!(observed["status"], "awaiting_files");
				}
				if case == "cancelled" {
					let cancelled = runner(
						&c,
						reqwest::Method::POST,
						&format!("/v1/operations/{operation}/cancel"),
						None,
					)
					.await;
					assert_eq!(cancelled["status"], "cancelled");
				}
				if !staged {
					let wanted = if case == "failed" {
						"failed"
					} else {
						"running"
					};
					let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
					loop {
						let observed = runner(
							&c,
							reqwest::Method::GET,
							&format!("/v1/operations/{operation}"),
							None,
						)
						.await;
						if observed["status"] == wanted {
							break;
						}
						assert!(tokio::time::Instant::now() < deadline, "{observed}");
						tokio::time::sleep(std::time::Duration::from_millis(100)).await;
					}
				}
				expected_runner = if case == "failed" {
					"failed"
				} else {
					"cancelled"
				};
			}
			sqlx::query(
				&Query::update()
					.table(Alias::new("core_records"))
					.value(Alias::new("data"), Expr::cust("$2"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
					.to_string(PostgresQueryBuilder),
			)
			.bind(id)
			.bind(data)
			.execute(&c.f.store.pool)
			.await
			.unwrap();
		}
		if disabled {
			let mut profile = (*c.f.store.capabilities.0).clone();
			profile.admission = false;
			c.f.store.capabilities = Runtime::new(profile).unwrap();
			c.app = aidash::api::router(c.f.clone());
		}
		ExtractionFixture {
			c,
			path,
			operation,
			expected: if disabled {
				"admission_disabled"
			} else {
				"extraction_failed"
			},
			expected_runner,
		}
	}
}

#[rstest::rstest]
#[case("failed")]
#[case("cancelled")]
#[case("disabled_undispatched")]
#[case("disabled_intent")]
#[case("disabled_staged")]
#[case("disabled_running")]
#[tokio::test]
async fn extraction_releases_terminal_runner_payloads_and_rollback_starts_no_new_parser(
	#[case] case: &'static str,
	#[with(case)]
	#[future]
	extraction_lifecycle_fixture: ExtractionFixture,
) {
	let f = Box::pin(extraction_lifecycle_fixture).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		f.c.f.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
	loop {
		let (status, state) = request(&f.c.app, &f.c.token, "GET", &f.path, Value::Null).await;
		assert_eq!(status, 200, "{state}");
		if state["state"] == "ready" {
			assert_eq!(state["extraction_state"], f.expected, "{case}: {state}");
			assert!(state["extraction"].is_null());
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{case}: {state}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	loop {
		let observed = runner(
			&f.c,
			reqwest::Method::GET,
			&format!("/v1/operations/{}", f.operation),
			None,
		)
		.await;
		assert_eq!(observed["status"], f.expected_runner, "{case}: {observed}");
		if f.expected_runner == "absent" {
			break;
		}
		if observed["acknowledged"] == true {
			assert_eq!(observed["termination_confirmed"], true);
			assert_eq!(observed["stdout"], "");
			assert_eq!(observed["files"], json!([]));
			assert_eq!(observed["displays"], json!([]));
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"{case}: acknowledgement missing"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	let (status, original) = request(
		&f.c.app,
		&f.c.token,
		"GET",
		&format!("{}/download", f.path),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{original}");
	assert_eq!(
		STANDARD.decode(original["data"].as_str().unwrap()).unwrap(),
		b"original bytes stay available"
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	f.c.close().await;
}
