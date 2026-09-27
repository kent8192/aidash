use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

struct DispatchFixture {
	c: CoreFixture,
	id: Uuid,
	path: String,
	bytes: Vec<u8>,
	valid: Runtime,
}

#[rstest::fixture]
fn dispatch_fixture(
	#[default("mismatch")] case: &str,
	#[future] runtime_fixture: CoreFixture,
) -> impl std::future::Future<Output = DispatchFixture> {
	let runtime_fixture = Box::pin(runtime_fixture);
	async move {
		let mut c = runtime_fixture.await;
		let valid = c.f.store.capabilities.clone();
		let mut profile = (*valid.0).clone();
		if case == "mismatch" {
			profile.cpu += 1;
		} else {
			profile.output_bytes = 4096;
		}
		c.f.store.capabilities = Runtime::new(profile).unwrap();
		c.app = aidash::api::router(c.f.clone());
		let bytes = "東京\t\u{0001}\n".repeat(2000).into_bytes();
		let (status, upload) = request(&c.app, &c.token, "POST", "/api/references/uploads", json!({"idempotency_key":Uuid::new_v4(),"name":"bounded.txt","media_type":"text/plain","size":bytes.len(),"digest":aidash::capabilities::objects::digest(&bytes)})).await;
		assert_eq!(status, 200, "{upload}");
		let id: Uuid = serde_json::from_value(upload["reference_id"].clone()).unwrap();
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_records"))
				.value(Alias::new("expires_at"), Expr::cust("$2"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(chrono::Utc::now() + chrono::Duration::seconds(3))
		.execute(&c.f.store.pool)
		.await
		.unwrap();
		let path = format!("/api/references/{id}");
		assert_eq!(
			request(
				&c.app,
				&c.token,
				"POST",
				&format!("{path}/chunks"),
				json!({"offset":0,"data":STANDARD.encode(&bytes)})
			)
			.await
			.0,
			200
		);
		assert_eq!(
			request(
				&c.app,
				&c.token,
				"POST",
				&format!("{path}/commit"),
				Value::Null
			)
			.await
			.0,
			200
		);
		DispatchFixture {
			c,
			id,
			path,
			bytes,
			valid,
		}
	}
}

async fn record(f: &DispatchFixture) -> Value {
	sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(f.id)
	.fetch_one(&f.c.f.store.pool)
	.await
	.unwrap()
}

#[rstest::rstest]
#[case("mismatch")]
#[case("output")]
#[tokio::test]
async fn extraction_requires_current_runner_limits_and_respects_lower_output_budget(
	#[case] case: &str,
	#[with(case)]
	#[future]
	dispatch_fixture: DispatchFixture,
) {
	use super::extraction_lifecycle_tests::runner;
	let mut f = Box::pin(dispatch_fixture).await;
	let expiry: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("expires_at"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(f.id)
	.fetch_one(&f.c.f.store.pool)
	.await
	.unwrap();
	assert!(
		expiry.is_none(),
		"committed reference cannot retain an upload expiry"
	);
	let (stop, rx) = tokio::sync::watch::channel(false);
	let mut worker = tokio::spawn(aidash::capabilities::operations::run(
		f.c.f.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
	if case == "mismatch" {
		loop {
			let (_, state) = request(&f.c.app, &f.c.token, "GET", &f.path, Value::Null).await;
			if state["extraction_state"] == "runtime_unavailable" {
				break;
			}
			assert!(tokio::time::Instant::now() < deadline, "{state}");
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
		let data = record(&f).await;
		assert!(data["instance"].is_null());
		let operation = data["operation_id"].as_str().unwrap();
		assert_eq!(
			runner(
				&f.c,
				reqwest::Method::GET,
				&format!("/v1/operations/{operation}"),
				None
			)
			.await["status"],
			"absent"
		);
		stop.send(true).unwrap();
		worker.await.unwrap().unwrap();
		f.c.f.store.capabilities = f.valid.clone();
		f.c.app = aidash::api::router(f.c.f.clone());
		stop.send_replace(false);
		worker = tokio::spawn(aidash::capabilities::operations::run(
			f.c.f.store.clone(),
			stop.subscribe(),
		));
	}
	loop {
		let (_, state) = request(&f.c.app, &f.c.token, "GET", &f.path, Value::Null).await;
		if state["state"] == "ready" {
			assert_eq!(
				state["extraction_state"],
				if case == "output" {
					"text_limit"
				} else {
					"ready"
				},
				"{state}"
			);
			if case == "output" {
				assert!(state["extraction"]["size"].as_u64().unwrap() <= 640);
			}
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{state}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	let data = record(&f).await;
	let operation = data["operation_id"].as_str().unwrap();
	loop {
		let state = runner(
			&f.c,
			reqwest::Method::GET,
			&format!("/v1/operations/{operation}"),
			None,
		)
		.await;
		if state["acknowledged"] == true {
			assert_eq!(state["stdout"], "");
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{state}");
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
		f.bytes
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	f.c.close().await;
}
