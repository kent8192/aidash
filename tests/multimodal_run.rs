mod common;

use aidash::{api, harness::Harness};
use axum::{
	Json, Router,
	body::Body,
	http::Request,
	routing::{get, post},
};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::mpsc;
use tower::ServiceExt;
use uuid::Uuid;

async fn upload(
	app: &Router,
	token: &str,
	workspace: &str,
	name: &str,
	mime: &str,
	bytes: &[u8],
) -> Value {
	let path = format!(
		"/api/workspaces/{workspace}/attachments?filename={name}&media_type={}&idempotency_key={}",
		mime.replace('/', "%2F"),
		Uuid::new_v4()
	);
	let response = app
		.clone()
		.oneshot(
			Request::post(path)
				.header("authorization", format!("Bearer {token}"))
				.header("content-type", "application/octet-stream")
				.body(Body::from(bytes.to_vec()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status();
	let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
		.await
		.unwrap();
	let body: Value = serde_json::from_slice(&bytes).unwrap();
	assert_eq!(status, 200, "{body}");
	body
}

#[rstest::rstest]
#[tokio::test]
async fn human_media_only_run_input_reaches_the_first_model_request_in_order(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (sent, mut received) = mpsc::unbounded_channel();
	let provider = Router::new()
		.route("/v1/models/fixture/endpoints", get(|| async { Json(json!({"data":{"architecture":{"input_modalities":["text","image","audio"]},"endpoints":[{"tag":"fixture/verified","context_length":128000}]}})) }))
		.route("/v1/endpoints/zdr", get(|| async { Json(json!({"data":[{"model_id":"fixture","tag":"fixture/verified"}]})) }))
		.route("/v1/chat/completions", post(move |Json(body): Json<Value>| {
			let sent = sent.clone();
			async move {
				sent.send(body).unwrap();
				Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"Done"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}}))
			}
		}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
	let (f, url, schema) = setup(&environment).await;
	let app = api::router(f.clone());
	let (mut policy, token, _) = bootstrap(&f, &app, &endpoint).await;
	policy["subjects"][aidash::domain::qualified_agent(&f.config.node_id, "research", "1.0.1")] =
		json!({"kind":"agent"});
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut model = f.registry.get("model", "1.0.0").await.unwrap();
	model.version = "1.0.1".into();
	model.config["modalities"] = json!(["text", "image", "audio"]);
	model.config["media_routes"] = json!([{
		"tag":"fixture/verified", "formats":["image/png", "wav"],
		"source":"mock provider contract", "verified_at": chrono::Utc::now() - chrono::Duration::hours(1),
		"expires_at": chrono::Utc::now() + chrono::Duration::hours(1)
	}]);
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/registry",
		json!(model),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"model","version":"1.0.1"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut agent = f.registry.get("research", "1.0.0").await.unwrap();
	agent.version = "1.0.1".into();
	agent.config["model"]["version"] = json!("1.0.1");
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/registry",
		json!(agent),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":"1.0.1"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({"title":"Media input","goal":"Inspect media","target":{"id":"research","version":"1.0.1"},"target_kind":"agent"})).await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let workspace = run.workspace_id.to_string();
	let bad = upload(
		&app,
		&token,
		&workspace,
		"bad.png",
		"image/png",
		b"not a png",
	)
	.await;
	let path = format!("/api/runs/{}/message", run.id);
	let (status, rejected) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":[bad["id"]]}),
	)
	.await;
	assert_eq!(status, 400, "{rejected}");
	assert!(f.store.run_inputs(run.id).await.unwrap().is_empty());
	let mut large_audio = vec![0_u8; 1024 * 1024];
	large_audio[..12].copy_from_slice(b"RIFF\0\0\0\0WAVE");
	let first_large = upload(
		&app,
		&token,
		&workspace,
		"large-a.wav",
		"audio/wav",
		&large_audio,
	)
	.await;
	let second_large = upload(
		&app,
		&token,
		&workspace,
		"large-b.wav",
		"audio/wav",
		&large_audio,
	)
	.await;
	let (status, rejected) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"","idempotency_key":Uuid::new_v4(),"attachment_ids":[first_large["id"],second_large["id"]]}),
	)
	.await;
	assert_eq!(status, 400, "{rejected}");
	assert!(f.store.run_inputs(run.id).await.unwrap().is_empty());
	let audio = upload(
		&app,
		&token,
		&workspace,
		"sample.wav",
		"audio/wav",
		b"RIFF\0\0\0\0WAVEaudio",
	)
	.await;
	let image = upload(
		&app,
		&token,
		&workspace,
		"sample.png",
		"image/png",
		b"\x89PNG\r\n\x1a\nimage",
	)
	.await;
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/message", Uuid::new_v4()),
			json!({"content":"","idempotency_key":Uuid::new_v4(),"attachment_ids":[image["id"]]}),
		)
		.await
		.0,
		403
	);
	let key = Uuid::new_v4();
	let input =
		json!({"content":"", "idempotency_key":key, "attachment_ids":[audio["id"],image["id"]]});
	let (status, body) = request(&app, &token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		request(&app, &token, "POST", &path, input.clone()).await.0,
		200
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&path,
			json!({"content":"","idempotency_key":key,"attachment_ids":[]}),
		)
		.await
		.0,
		409
	);
	let mut reversed = input;
	reversed["attachment_ids"] = json!([image["id"], audio["id"]]);
	assert_eq!(request(&app, &token, "POST", &path, reversed).await.0, 409);
	let worker = Harness {
		federation: f.clone(),
	};
	let mut saw_subject_media = false;
	for _ in 0..6 {
		assert!(worker.worker_once().await.unwrap());
		if let Ok(body) = received.try_recv() {
			let parts = body["messages"][1]["content"].as_array().unwrap();
			let types: Vec<_> = parts
				.iter()
				.map(|part| part["type"].as_str().unwrap())
				.collect();
			assert!(
				types.ends_with(&["text", "input_audio", "text", "image_url"]),
				"{types:?}"
			);
			assert_eq!(body["provider"]["only"], json!(["fixture/verified"]));
			saw_subject_media = true;
			break;
		}
	}
	assert!(
		saw_subject_media,
		"model request did not include the accepted run media"
	);
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "TOOL_CALL");
	let (status, correction) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"Please reconsider the media", "idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{correction}");
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "THINKING");
	assert!(worker.worker_once().await.unwrap());
	let retried = received.try_recv().expect("stale media was not reinferred");
	let retried_types: Vec<_> = retried["messages"][1]["content"]
		.as_array()
		.unwrap()
		.iter()
		.map(|part| part["type"].as_str().unwrap())
		.collect();
	assert!(
		retried_types.ends_with(&["text", "input_audio", "text", "image_url"]),
		"{retried_types:?}"
	);
	for _ in 0..4 {
		if f.store.run(run.id).await.unwrap().phase == "COMPLETED" {
			break;
		}
		assert!(worker.worker_once().await.unwrap());
	}
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "COMPLETED");
	let operator = f.config.api_token.clone();
	let (status, created) = request(
		&app,
		&operator,
		"POST",
		"/api/conversations",
		json!({"title":"Operator media", "goal":"Inspect image", "target":{"id":"research","version":"1.0.1"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let operator_workspace = created["workspace"]["id"].as_str().unwrap();
	let operator_run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == operator_workspace)
		.unwrap();
	let operator_image = upload(
		&app,
		&operator,
		operator_workspace,
		"operator.png",
		"image/png",
		b"\x89PNG\r\n\x1a\noperator",
	)
	.await;
	let (status, sent) = request(
		&app,
		&operator,
		"POST",
		&format!("/api/runs/{}/message", operator_run.id),
		json!({"content":"","idempotency_key":Uuid::new_v4(),"attachment_ids":[operator_image["id"]]}),
	)
	.await;
	assert_eq!(status, 200, "{sent}");
	let mut saw_operator_media = false;
	for _ in 0..6 {
		let worked = worker.worker_once().await.unwrap();
		assert!(worked);
		while let Ok(body) = received.try_recv() {
			let Some(parts) = body["messages"][1]["content"].as_array() else {
				continue;
			};
			let types: Vec<_> = parts
				.iter()
				.map(|part| part["type"].as_str().unwrap())
				.collect();
			if types == ["text", "text", "image_url"] {
				assert_eq!(body["provider"]["only"], json!(["fixture/verified"]));
				saw_operator_media = true;
				break;
			}
		}
		if saw_operator_media {
			break;
		}
	}
	let final_operator_run = f.store.run(operator_run.id).await.unwrap();
	assert!(
		saw_operator_media,
		"operator run did not infer its accepted media: {} {} {:?} {:?} lease={:?} until={:?} task={:?}",
		final_operator_run.phase,
		final_operator_run.control,
		final_operator_run.error,
		final_operator_run.pending,
		final_operator_run.lease_owner,
		final_operator_run.lease_until,
		f.store
			.task(final_operator_run.task_id)
			.await
			.unwrap()
			.status
	);
	server.abort();
	cleanup(f, &url, &schema).await;
}
