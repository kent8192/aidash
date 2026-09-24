mod common;

use aidash::{api, harness::Harness};
use axum::{Json, Router, routing::post};
use common::{bootstrap, cleanup, request, setup};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn messages_accepted_during_and_after_inference_are_seen_before_completion() {
	let entered = Arc::new(Notify::new());
	let release = Arc::new(Notify::new());
	let calls = Arc::new(AtomicUsize::new(0));
	let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
	let provider = Router::new().route(
		"/v1/chat/completions",
		post({
			let entered = entered.clone();
			let release = release.clone();
			let calls = calls.clone();
			let requests = requests.clone();
			move |Json(body): Json<Value>| {
				let entered = entered.clone();
				let release = release.clone();
				let calls = calls.clone();
				let requests = requests.clone();
				async move {
					let call = calls.fetch_add(1, Ordering::SeqCst);
					requests.lock().await.push(body);
					if call == 0 {
						entered.notify_one();
						release.notified().await;
					}
					let text = match call {
						0 => "stale first answer",
						1 => "stale second answer",
						_ => "answer with both corrections",
					};
					Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":text}}],"usage":{"prompt_tokens":10,"completion_tokens":10}}))
				}
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, &endpoint).await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Finalization race","goal":"Give a short answer","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let worker = Harness {
		federation: f.clone(),
	};
	assert!(worker.worker_once().await.unwrap());
	let first = tokio::spawn({
		let worker = worker.clone();
		async move { worker.worker_once().await }
	});
	tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
		.await
		.expect("provider did not receive first inference");
	let first_key = Uuid::new_v4();
	let path = format!("/api/runs/{}/message", run.id);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"first correction","idempotency_key":first_key}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	release.notify_one();
	assert!(first.await.unwrap().unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "THINKING");
	tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "TOOL_CALL");
	let second_key = Uuid::new_v4();
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"second correction","idempotency_key":second_key}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "THINKING");
	assert!(worker.worker_once().await.unwrap());
	let lease_token = Uuid::new_v4();
	let leased = f.store.lease_run(lease_token, 30).await.unwrap().unwrap();
	assert_eq!(leased.id, run.id);
	assert!(
		f.store
			.begin_final_completion(&leased, lease_token)
			.await
			.unwrap()
	);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"during final commit","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	f.store.release_lease(run.id, lease_token).await.unwrap();
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "COMPLETED");
	assert_eq!(f.store.task(run.task_id).await.unwrap().status, "COMPLETED");
	let seen = requests.lock().await;
	assert_eq!(seen.len(), 3);
	assert!(!seen[0].to_string().contains("first correction"));
	assert!(seen[1].to_string().contains("first correction"));
	assert!(seen[2].to_string().contains("first correction"));
	assert!(seen[2].to_string().contains("second correction"));
	drop(seen);
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	assert_eq!(snapshot.artifacts.len(), 1);
	assert_eq!(
		snapshot.artifacts[0].content,
		"answer with both corrections"
	);
	assert!(
		snapshot
			.messages
			.iter()
			.any(|message| message.content == "answer with both corrections")
	);
	assert!(
		!snapshot
			.messages
			.iter()
			.any(|message| message.content.starts_with("stale"))
	);
	assert!(
		!snapshot
			.messages
			.iter()
			.any(|message| message.content == "during final commit")
	);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"too late","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	assert!(body["error"].as_str().unwrap().contains("not accepted"));
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&path,
			json!({"content":"first correction","idempotency_key":first_key}),
		)
		.await
		.0,
		200
	);
	server.abort();
	cleanup(f, &url, &schema).await;
}
