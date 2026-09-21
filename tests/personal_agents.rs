mod common;
use aidash::{api, registry::Entry};
use axum::{
	body::{Body, to_bytes},
	http::Request,
};
use common::*;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

async fn personal(app: &axum::Router, token: &str, key: Uuid, value: Value) -> (u16, Value) {
	let response = app
		.clone()
		.oneshot(
			Request::builder()
				.method("POST")
				.uri("/api/agents/personal")
				.header("authorization", format!("Bearer {token}"))
				.header("content-type", "application/json")
				.header("idempotency-key", key.to_string())
				.body(Body::from(value.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let bytes = to_bytes(response.into_body(), 1_048_576).await.unwrap();
	(status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn private_documents_are_atomic_idempotent_and_absent_from_registry() {
	let (f, url, schema) = setup().await;
	let received = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
	let capture = received.clone();
	let provider = axum::Router::new().route("/v1/chat/completions", axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let capture = capture.clone();
        async move {
            *capture.lock().unwrap() = Some(body);
            axum::Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Complete"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
        }
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, provider).await.unwrap();
	});
	let app = api::router(f.clone());
	let (_, subject_token, _) = bootstrap(&f, &app, &endpoint).await;
	let skill = json!({"id":"writing","version":"1.0.0","kind":"skill","name":{"en":"Writing"},"description":{"en":"Writing"},"config":{"instructions":"Use the provided reference documents."}});
	assert_eq!(
		request(&app, &f.config.api_token, "POST", "/api/registry", skill)
			.await
			.0,
		200
	);
	let input = json!({"entry":{"id":"","version":"1.0.0","kind":"agent","name":{"en":"Personal"},"description":{"en":"Skills first"},"config":{"model":{"id":"model","version":"1.0.0"},"skills":[{"id":"writing","version":"1.0.0"}]}},"documents":[{"name":"private.pdf","media_type":"application/pdf","text":"PRIVATE-REFERENCE-123"}]});
	let key = Uuid::new_v4();
	assert_eq!(
		personal(&app, &subject_token, key, input.clone()).await.0,
		403
	);
	let first = personal(&app, &f.config.api_token, key, input.clone()).await;
	assert_eq!(first.0, 200, "{first:?}");
	assert!(!first.1.to_string().contains("PRIVATE-REFERENCE"));
	assert_eq!(
		first,
		personal(&app, &f.config.api_token, key, input.clone()).await
	);
	let entry: Entry = serde_json::from_value(first.1.clone()).unwrap();
	let documents = aidash::knowledge::load(&f.registry.db, &entry)
		.await
		.unwrap();
	assert_eq!(documents[0]["text"], "PRIVATE-REFERENCE-123");
	let mut changed = input.clone();
	changed["documents"][0]["text"] = json!("Different");
	assert_eq!(
		personal(&app, &f.config.api_token, key, changed).await.0,
		409
	);
	let (_, state) = request(&app, &f.config.api_token, "GET", "/api/state", Value::Null).await;
	assert!(!state.to_string().contains("PRIVATE-REFERENCE"));
	assert!(!state.to_string().contains("private.pdf"));
	let mut cloned = entry.clone();
	cloned.id = "cloned-personal".into();
	f.registry.register(cloned.clone()).await.unwrap();
	assert!(
		aidash::knowledge::load(&f.registry.db, &cloned)
			.await
			.is_err()
	);
	let mut invalid = input;
	invalid["documents"][0]["text"] = json!(" ");
	assert_eq!(
		personal(&app, &f.config.api_token, Uuid::new_v4(), invalid)
			.await
			.0,
		400
	);
	let workspace = f
		.store
		.create_workspace("Personal", "Use documents")
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			workspace.id,
			&aidash::domain::NewTask {
				title: "Read documents".into(),
				description: "Summarize references".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let claimed = request(
		&app,
		&f.config.api_token,
		"POST",
		&format!("/api/tasks/{}/claim", task.id),
		json!({"revision":0,"agent":{"id":entry.id,"version":entry.version}}),
	)
	.await;
	assert_eq!(claimed.0, 200, "{claimed:?}");
	let harness = aidash::harness::Harness {
		federation: f.clone(),
	};
	for _ in 0..12 {
		if !harness.worker_once().await.unwrap() {
			break;
		}
	}
	let body = received
		.lock()
		.unwrap()
		.clone()
		.expect("model received inference request");
	let context: Value =
		serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
	assert_eq!(
		context["current"]["reference_documents"][0]["text"],
		"PRIVATE-REFERENCE-123"
	);
	let instructions = body["messages"][0]["content"].as_str().unwrap();
	assert!(instructions.contains("Use the provided reference documents."));
	assert!(!instructions.contains("PRIVATE-REFERENCE-123"));
	server.abort();
	cleanup(f, &url, &schema).await;
}
