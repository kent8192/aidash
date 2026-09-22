mod common;

use aidash::{api, harness::Harness};
use common::*;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	sync::{Notify, oneshot},
	task::JoinSet,
	time::timeout,
};

struct StalledProvider {
	endpoint: String,
	entered: oneshot::Receiver<()>,
	disconnected: oneshot::Receiver<()>,
	tasks: JoinSet<()>,
}

impl StalledProvider {
	async fn start(stall_body: bool) -> Self {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let (entered_tx, entered) = oneshot::channel();
		let (disconnected_tx, disconnected) = oneshot::channel();
		let mut tasks = JoinSet::new();
		tasks.spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut request = Vec::new();
			let mut buffer = [0_u8; 4096];
			// Consume the actual HTTP request before observing cancellation.
			loop {
				let n = socket.read(&mut buffer).await.unwrap();
				assert!(n > 0, "provider request closed before its body arrived");
				request.extend_from_slice(&buffer[..n]);
				assert!(request.len() <= 1_048_576);
				if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
					let headers = std::str::from_utf8(&request[..end]).unwrap();
					let length: usize = headers
						.lines()
						.filter_map(|line| line.split_once(':'))
						.find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
						.expect("JSON request must have a content length")
						.1
						.trim()
						.parse()
						.unwrap();
					if request.len() >= end + 4 + length {
						let body: Value =
							serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
						assert_eq!(body["model"], "fixture");
						break;
					}
				}
			}
			if stall_body {
				// Headers succeed, but the provider never finishes its JSON body.
				socket
					.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n{")
					.await
					.unwrap();
			}
			entered_tx.send(()).unwrap();
			let n = socket.read(&mut buffer).await.unwrap();
			assert_eq!(n, 0, "cancellation must close the pending HTTP request");
			disconnected_tx.send(()).unwrap();
		});
		Self {
			endpoint,
			entered,
			disconnected,
			tasks,
		}
	}
}

async fn cancel_stalled_inference(scoped: bool, stall_body: bool) {
	let (mut f, url, schema) = setup().await;
	// Cancellation must not wait for the next (100-second) lease heartbeat.
	f.config.lease_seconds = 300;
	let mut server = StalledProvider::start(stall_body).await;
	let mut controller = f.clone();
	// Separate notification objects model distinct API and worker processes.
	controller.notify = Arc::new(Notify::new());
	let app = api::router(controller);
	let (_, subject_token, scoped_task) = bootstrap(&f, &app, &server.endpoint).await;
	let (token, task_id) = if scoped {
		(subject_token, scoped_task.to_string())
	} else {
		let token = f.config.api_token.clone();
		let (status, workspace) = request(
			&app,
			&token,
			"POST",
			"/api/workspaces",
			json!({"title":"Legacy cancellation","goal":"Cancel inference"}),
		)
		.await;
		assert_eq!(status, 200, "{workspace}");
		let (status, task) = request(
			&app,
			&token,
			"POST",
			&format!(
				"/api/workspaces/{}/tasks",
				workspace["id"].as_str().unwrap()
			),
			json!({"title":"Cancel inference","description":"No output expected"}),
		)
		.await;
		assert_eq!(status, 200, "{task}");
		(token, task["id"].as_str().unwrap().to_owned())
	};
	let (status, claimed) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase, "THINKING");
	let mut workers = JoinSet::new();
	let worker = harness.clone();
	workers.spawn(async move { worker.worker_once().await });
	timeout(Duration::from_secs(10), &mut server.entered)
		.await
		.expect("worker must reach the provider")
		.unwrap();
	let (status, cancelled) = timeout(
		Duration::from_secs(5),
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/control", run.id),
			json!({"action":"cancel"}),
		),
	)
	.await
	.expect("cancellation must not be blocked by inference");
	assert_eq!(status, 200, "{cancelled}");
	assert_eq!(cancelled["control"], "CANCELLED");
	assert!(
		timeout(Duration::from_secs(5), workers.join_next())
			.await
			.expect("cancelled inference must release the worker promptly")
			.unwrap()
			.unwrap()
			.unwrap()
	);
	timeout(Duration::from_secs(5), &mut server.disconnected)
		.await
		.expect("cancelled inference must drop its HTTP connection")
		.unwrap();
	server.tasks.join_next().await.unwrap().unwrap();

	// Finish the existing durable cancellation path without another inference.
	for _ in 0..3 {
		if f.store.run(run.id).await.unwrap().phase == "CANCELLED" {
			break;
		}
		assert!(harness.worker_once().await.unwrap());
	}
	let cancelled = f.store.run(run.id).await.unwrap();
	assert_eq!(cancelled.phase, "CANCELLED");
	assert_eq!(cancelled.control, "CANCELLED");
	assert!(cancelled.pending.get("retry_at").is_none());
	assert!(cancelled.pending.get("response").is_none());
	assert_eq!(f.store.task(run.task_id).await.unwrap().status, "CANCELLED");
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	assert!(snapshot.artifacts.is_empty());
	assert!(!snapshot.events.iter().any(|e| e.kind == "model.completed"));
	assert!(!harness.worker_once().await.unwrap());
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn scoped_cancellation_aborts_inference_before_response_headers() {
	cancel_stalled_inference(true, false).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn legacy_cancellation_aborts_inference_during_response_body() {
	cancel_stalled_inference(false, true).await;
}
