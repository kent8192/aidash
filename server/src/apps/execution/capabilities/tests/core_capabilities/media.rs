use super::*;
use aidash_server::harness::Harness;
use axum::{
	Json, Router,
	routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use std::sync::{
	Mutex,
	atomic::{AtomicUsize, Ordering},
};

struct ProviderTask(tokio::task::JoinHandle<()>);
impl Drop for ProviderTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}

#[rstest::rstest]
#[tokio::test]
async fn selected_file_media_preserves_order_and_duplicate_bytes_through_worker_inference(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	// Arrange: use a real HTTP/DB scope and owned working objects, with a local provider.
	let storage = tempfile::Builder::new()
		.prefix("aidash-selected-media-")
		.tempdir_in("/tmp")
		.unwrap();
	let calls = Arc::new(AtomicUsize::new(0));
	let selections = Arc::new(Mutex::new(Vec::<Value>::new()));
	let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
	let provider_calls = calls.clone();
	let provider_selections = selections.clone();
	let provider_requests = requests.clone();
	let provider=Router::new()
        .route("/v1/models/fixture/endpoints",get(||async{Json(json!({"data":{"architecture":{"input_modalities":["text","image"]},"endpoints":[{"tag":"fixture/selected","context_length":128000}]}}))}))
        .route("/v1/endpoints/zdr",get(||async{Json(json!({"data":[{"model_id":"fixture","tag":"fixture/selected"}]}))}))
        .route("/v1/chat/completions",post(move |Json(body):Json<Value>|{
            let first=provider_calls.fetch_add(1,Ordering::SeqCst)==0;
            provider_requests.lock().unwrap().push(body);
            let selections=provider_selections.lock().unwrap().clone();
            async move {
                if first {
                    let tool_calls=selections.into_iter().enumerate().map(|(i,arguments)|json!({"id":format!("selected-{i}"),"type":"function","function":{"name":"file_read","arguments":arguments.to_string()}})).collect::<Vec<_>>();
                    Json(json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":tool_calls}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}))
                } else {Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"Done"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}))}
            }
        }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let _provider = ProviderTask(tokio::spawn(async move {
		axum::serve(listener, provider).await.unwrap()
	}));
	let mut c = build_core_fixture_at(environment, "aidash://selected-media", &endpoint).await;
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.storage = storage.path().to_owned();
	c.root = storage.path().to_owned();
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = common::application(c.f.clone()).await;
	let mut model = c.f.registry.get("model", "1.0.0").await.unwrap();
	model.version = "1.1.0".into();
	model.config["modalities"] = json!(["text", "image"]);
	model.config["media_routes"] = json!([{"tag":"fixture/selected","formats":["image/png"],"source":"local provider contract","verified_at":chrono::Utc::now()-chrono::Duration::hours(1),"expires_at":chrono::Utc::now()+chrono::Duration::hours(1)}]);
	let mut agent = c.f.registry.get("research", "1.1.0").await.unwrap();
	agent.version = "1.1.1".into();
	agent.config["model"]["version"] = json!("1.1.0");
	c.policy["subjects"]
		[aidash_server::domain::qualified_agent(&c.f.config.node_id, "research", "1.1.1")] =
		json!({"kind":"agent"});
	let (status, body) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	for entry in [model, agent] {
		let (status, body) = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/registry",
			json!(&entry),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		let (status, body) = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":entry.id,"version":entry.version},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	let (status, body) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", c.task),
		json!({"node_id":c.f.config.node_id,"agent":{"id":"research","version":"1.1.1"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let run = c.f.store.runs().await.unwrap().remove(0);
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let images = [
		("images/東京.png", b"\x89PNG\r\n\x1a\nfirst".as_slice()),
		("images/second.png", b"\x89PNG\r\n\x1a\nsecond".as_slice()),
	];
	let mut files = vec![];
	for (path, bytes) in images {
		let file = Uuid::new_v4();
		let digest = aidash_server::capabilities::objects::digest(bytes);
		tokio::fs::write(storage.path().join(file.simple().to_string()), bytes)
			.await
			.unwrap();
		let query = Query::insert()
			.into_table(Alias::new("core_objects"))
			.columns(["id", "tenant", "area_id", "kind", "digest", "size"].map(Alias::new))
			.from_subquery((1..=6).map(|i| Expr::cust(format!("${i}"))).fold(
				Query::select(),
				|mut select, expr| {
					select.expr(expr);
					select
				},
			))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&query)
			.bind(file)
			.bind("acme")
			.bind(area_id)
			.bind("working")
			.bind(&digest)
			.bind(bytes.len() as i64)
			.execute(&c.f.store.pool)
			.await
			.unwrap();
		files.push(json!({"file_id":file,"path":path,"size":bytes.len(),"digest":digest,"media_type":"image/png","scope":"working","provenance":{"kind":"media-fixture"}}));
	}
	let query = Query::update()
		.table(Alias::new("core_areas"))
		.value_expr(Alias::new("manifest"), Expr::cust("$2"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query)
		.bind(area_id)
		.bind(json!(files))
		.execute(&c.f.store.pool)
		.await
		.unwrap();
	*selections.lock().unwrap()=[1,0,1].map(|i|json!({"file_id":files[i]["file_id"],"expected_digest":files[i]["digest"],"representation":"model_input"})).to_vec();
	// Act: real worker authority resolves each persisted selection and sends it to inference.
	let worker = Harness {
		federation: c.f.clone(),
	};
	for _ in 0..12 {
		if c.f.store.run(run.id).await.unwrap().phase().as_str() == "COMPLETED" {
			break;
		}
		assert!(worker.worker_once().await.unwrap());
	}
	// Assert: duplicate selections and labels survive both the native row boundary and use case.
	assert_eq!(
		c.f.store.run(run.id).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
	{
		let requests = requests.lock().unwrap();
		assert_eq!(requests.len(), 2, "{requests:?}");
		let parts = requests[1]["messages"][1]["content"].as_array().unwrap();
		let actual_images = parts
			.iter()
			.filter_map(|part| part["image_url"]["url"].as_str())
			.collect::<Vec<_>>();
		let expected_images =
			[1, 0, 1].map(|i| format!("data:image/png;base64,{}", STANDARD.encode(images[i].1)));
		assert_eq!(actual_images, expected_images);
		let labels = parts
			.iter()
			.filter_map(|part| part["text"].as_str())
			.filter(|text| text.starts_with("Selected file: "))
			.collect::<Vec<_>>();
		assert_eq!(
			labels,
			vec![
				"Selected file: images/second.png",
				"Selected file: images/東京.png",
				"Selected file: images/second.png"
			]
		);
		assert_eq!(requests[1]["provider"]["only"], json!(["fixture/selected"]));
	}
	c.close().await;
}
