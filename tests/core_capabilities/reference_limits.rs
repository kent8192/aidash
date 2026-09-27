use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

#[rstest::fixture]
fn bounded_reference_fixture(
	#[future] runtime_fixture: CoreFixture,
) -> impl std::future::Future<Output = CoreFixture> {
	// Box before constructing the next fixture future so nested setup does not
	// retain another copy of the large database/bootstrap future on the stack.
	let runtime_fixture = Box::pin(runtime_fixture);
	async move {
		let mut c = runtime_fixture.await;
		let mut profile = (*c.f.store.capabilities.0).clone();
		profile.limits.reference_text_bytes = 64;
		c.f.store.capabilities = Runtime::new(profile).unwrap();
		c.app = aidash::api::router(c.f.clone());
		c
	}
}

#[rstest::rstest]
#[tokio::test]
async fn reference_configuration_enforces_aggregate_and_lowered_limits_without_rewriting_versions(
	#[future] bounded_reference_fixture: CoreFixture,
) {
	let mut c = Box::pin(bounded_reference_fixture).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let mut bindings = vec![];
	for index in 0..3 {
		let bytes = "東京".repeat(if index == 2 { 20 } else { 5 }).into_bytes();
		let digest = aidash::capabilities::objects::digest(&bytes);
		let (status, upload) = request(&c.app,&c.token,"POST","/api/references/uploads",json!({"idempotency_key":Uuid::new_v4(),"name":format!("fixture-{index}.txt"),"media_type":"text/plain","size":bytes.len(),"digest":digest})).await;
		assert_eq!(status, 200, "{upload}");
		let path = format!(
			"/api/references/{}",
			upload["reference_id"].as_str().unwrap()
		);
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
		let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
		let ready = loop {
			let (_, ready) = request(&c.app, &c.token, "GET", &path, Value::Null).await;
			if ready["state"] == "ready" {
				break ready;
			}
			assert!(tokio::time::Instant::now() < deadline, "{ready}");
			tokio::time::sleep(std::time::Duration::from_millis(150)).await;
		};
		assert_eq!(
			ready["extraction_state"],
			if index == 2 { "text_limit" } else { "ready" }
		);
		assert!(ready["extraction"]["size"].as_u64().unwrap() <= 64);
		if index < 2 {
			bindings.push(json!({"reference_id":upload["reference_id"],"digest":digest}));
		}
	}
	let before = c.f.registry.get("research", "1.1.0").await.unwrap();
	let mut input = json!({"idempotency_key":Uuid::new_v4(),"source_version":"1.1.0","new_version":"1.2.0","core_capabilities":{"files":true},"skill_attachments":[],"skill_roots":[],"reference_attachments":bindings});
	let path = "/api/agents/research/capabilities";
	let (status, rejected) = request(&c.app, &c.token, "POST", path, input.clone()).await;
	assert_eq!(status, 400, "{rejected}");
	assert_eq!(rejected["error"]["code"], "REFERENCE_SET_LIMIT");
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.limits.reference_text_bytes = 65536;
	profile.limits.reference_files = 1;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	assert_eq!(
		request(&c.app, &c.token, "POST", path, input.clone())
			.await
			.0,
		400
	);
	input["reference_attachments"] = json!([bindings[0]]);
	let (status, saved) = request(&c.app, &c.token, "POST", path, input).await;
	assert_eq!(status, 200, "{saved}");
	assert_eq!(saved["entry"]["version"], "1.2.0");
	assert_eq!(
		saved["entry"]["config"]["reference_attachments"],
		json!([bindings[0]])
	);
	assert_eq!(c.f.registry.get("research", "1.1.0").await.unwrap(), before);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

struct ExtractionCase {
	bytes: Vec<u8>,
	expected: &'static str,
}
#[rstest::fixture]
fn extraction_case(#[default("blank")] kind: &str) -> ExtractionCase {
	let (bytes, expected) = match kind {
		"blank" => (
			include_bytes!("../fixtures/reference-errors/blank.pdf").to_vec(),
			"non_extractable",
		),
		"encrypted" => (
			include_bytes!("../fixtures/reference-errors/encrypted.pdf").to_vec(),
			"encrypted",
		),
		"pages" => (
			include_bytes!("../fixtures/reference-errors/too-many-pages.pdf").to_vec(),
			"page_limit",
		),
		"malformed" => (b"%PDF-1.7\nnot a PDF".to_vec(), "malformed"),
		"unsupported" => (vec![0, 255, 0, 128], "unsupported"),
		"text" => ("東京資料\n".repeat(10000).into_bytes(), "text_limit"),
		_ => unreachable!(),
	};
	ExtractionCase { bytes, expected }
}
#[rstest::rstest]
#[case("blank")]
#[case("encrypted")]
#[case("pages")]
#[case("malformed")]
#[case("unsupported")]
#[case("text")]
#[tokio::test]
async fn isolated_extraction_reports_limits_without_losing_authorized_original(
	#[future] runtime_fixture: CoreFixture,
	#[case] kind: &str,
	#[with(kind)] extraction_case: ExtractionCase,
) {
	let c = Box::pin(runtime_fixture).await;
	let ExtractionCase { bytes, expected } = extraction_case;
	let input = json!({"idempotency_key":Uuid::new_v4(),"name":"fixture.pdf","media_type":"application/pdf","size":bytes.len(),"digest":aidash::capabilities::objects::digest(&bytes)});
	let (status, upload) =
		request(&c.app, &c.token, "POST", "/api/references/uploads", input).await;
	assert_eq!(status, 200, "{upload}");
	let path = format!(
		"/api/references/{}",
		upload["reference_id"].as_str().unwrap()
	);
	let (status, uploaded) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{path}/chunks"),
		json!({"offset":0,"data":STANDARD.encode(&bytes)}),
	)
	.await;
	assert_eq!(status, 200, "{uploaded}");
	let (status, committed) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{path}/commit"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{committed}");
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);
	let ready = loop {
		let (status, value) = request(&c.app, &c.token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{value}");
		if value["state"] == "ready" {
			break value;
		}
		assert!(tokio::time::Instant::now() < deadline, "{value}");
		tokio::time::sleep(std::time::Duration::from_millis(150)).await;
	};
	assert_eq!(ready["extraction_state"], expected, "{kind}: {ready}");
	if expected == "text_limit" {
		assert!(ready["extraction"]["size"].as_u64().unwrap() <= 65536);
	} else {
		assert!(ready["extraction"].is_null());
	}
	let (status, original) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("{path}/download"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{original}");
	assert_eq!(
		STANDARD.decode(original["data"].as_str().unwrap()).unwrap(),
		bytes
	);
	let (status,oversized)=request(&c.app,&c.token,"POST","/api/references/uploads",json!({"idempotency_key":Uuid::new_v4(),"name":"too-big.pdf","media_type":"application/pdf","size":(10<<20)+1,"digest":"a".repeat(64)})).await;
	assert_eq!(status, 400, "{oversized}");
	assert_eq!(oversized["error"]["code"], "REFERENCE_UPLOAD_LIMIT");
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
