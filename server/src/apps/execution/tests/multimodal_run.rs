#[path = "support/upstream.rs"]
mod upstream_fixtures;
use reinhardt::ServerRouter as Router;
use reinhardt::test::fixtures::server::TestServerGuard;
use upstream_fixtures::{handler, upstream};
#[path = "support/legacy.rs"]
mod common;

use aidash_server::{federation::Federation, harness::Harness};

use base64::Engine;
use common::{bootstrap, cleanup, request};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use tokio::sync::mpsc;

use uuid::Uuid;

async fn upload(
	app: &common::TestApplication,
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

	let authorization = format!("Bearer {token}");
	let response = app
		.api_http
		.post_raw_with_headers(
			&path,
			bytes,
			"application/octet-stream",
			&[("authorization", authorization.as_str())],
		)
		.await
		.unwrap();
	let status = response.status_code();
	let body = response.json_value().unwrap();
	assert_eq!(status, 200, "{body}");

	body
}

struct AudioBatchCase<'a> {
	first_audio: &'a [u8],
	message_text: &'a str,
	leading_text: Option<&'a str>,
}

async fn verify_audio_batches(
	app: &common::TestApplication,
	f: &Federation,
	worker: &Harness,
	received: &mut mpsc::UnboundedReceiver<Value>,
	operator: &str,
	case: AudioBatchCase<'_>,
) {
	let (status, audio_created) = request(
		app,
		operator,
		"POST",
		"/api/conversations",
		json!({"title":"Audio batching", "goal":"Inspect both clips", "target":{"id":"research","version":"1.0.1"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{audio_created}");
	let audio_workspace = audio_created["workspace"]["id"].as_str().unwrap();
	let audio_run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == audio_workspace)
		.unwrap();
	if let Some(content) = case.leading_text {
		let (status, sent) = request(
			app,
			operator,
			"POST",
			&format!("/api/runs/{}/message", audio_run.id),
			json!({"content":content,"idempotency_key":Uuid::new_v4()}),
		)
		.await;
		assert_eq!(status, 200, "{sent}");
	}
	let mut second_audio = case.first_audio.to_vec();
	*second_audio.last_mut().unwrap() = 1;
	let first = upload(
		app,
		operator,
		audio_workspace,
		"first.wav",
		"audio/wav",
		case.first_audio,
	)
	.await;
	let second = upload(
		app,
		operator,
		audio_workspace,
		"second.wav",
		"audio/wav",
		&second_audio,
	)
	.await;
	for attachment in [first, second] {
		let (status, sent) = request(
			app,
			operator,
			"POST",
			&format!("/api/runs/{}/message", audio_run.id),
			json!({"content":case.message_text, "idempotency_key":Uuid::new_v4(), "attachment_ids":[attachment["id"]]}),
		)
		.await;
		assert_eq!(status, 200, "{sent}");
	}
	let mut audio_batches = Vec::new();
	let mut saw_prefix_batch = false;
	for _ in 0..12 {
		assert!(worker.worker_once().await.unwrap());
		while let Ok(body) = received.try_recv() {
			let Some(parts) = body["messages"][1]["content"].as_array() else {
				if let Some(prefix) = case.leading_text
					&& body["messages"][1]["content"]
						.as_str()
						.is_some_and(|content| content.contains(prefix))
				{
					assert!(body.get("tools").is_none());
					saw_prefix_batch = true;
				}
				continue;
			};
			let audio: Vec<String> = parts
				.iter()
				.filter_map(|part| part["input_audio"]["data"].as_str().map(str::to_owned))
				.collect();
			if !audio.is_empty() {
				assert_eq!(audio.len(), 1, "audio messages must fit individually");
				audio_batches.push(audio[0].clone());
			}
		}
		if audio_batches.len() == 2 {
			break;
		}
	}
	assert_eq!(
		audio_batches,
		vec![
			base64::engine::general_purpose::STANDARD.encode(case.first_audio),
			base64::engine::general_purpose::STANDARD.encode(&second_audio),
		]
	);
	if case.leading_text.is_some() {
		assert!(
			saw_prefix_batch,
			"the leading text must be deferred separately"
		);
	}
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(
		f.store.run(audio_run.id).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
}

async fn verify_attachment_labels_count_toward_admission(
	app: &common::TestApplication,
	f: &Federation,
	operator: &str,
) {
	let (status, created) = request(
		app,
		operator,
		"POST",
		"/api/conversations",
		json!({"title":"Media label budget", "goal":"Inspect audio", "target":{"id":"research","version":"1.0.1"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let workspace = created["workspace"]["id"].as_str().unwrap();
	let run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == workspace)
		.unwrap();
	let seq = f
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.map_or(1, |input| input.seq + 1);
	let short_name = "s.wav";
	let long_name = format!("{}.wav", "x".repeat(251));
	let short_label = format!("Run message {seq} attachment: {short_name}");
	let text_body = json!({"messages":[
		{"role":"system","content":""},
		{"role":"user","content":[
			{"type":"text","text":json!({"run_message":""}).to_string()},
			{"type":"text","text":short_label}
		]}
	]});
	// Leave a little room for the short label while the 255-byte filename
	// pushes the otherwise identical audio beyond the admission boundary.
	let audio_tokens = (f.run_message_limit(&run).await.unwrap() * 4)
		.checked_sub(text_body.to_string().len() + 2 * 1024 + 160)
		.unwrap();
	let mut audio = vec![0_u8; audio_tokens * 16];
	audio[..12].copy_from_slice(b"RIFF\0\0\0\0WAVE");
	let long = upload(app, operator, workspace, &long_name, "audio/wav", &audio).await;
	let short = upload(app, operator, workspace, short_name, "audio/wav", &audio).await;
	let path = format!("/api/runs/{}/message", run.id);
	let (status, rejected) = request(
		app,
		operator,
		"POST",
		&path,
		json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":[long["id"]]}),
	)
	.await;
	assert_eq!(status, 400, "{rejected}");
	assert!(f.store.run_inputs(run.id).await.unwrap().is_empty());
	let (status, sent) = request(
		app,
		operator,
		"POST",
		&path,
		json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":[short["id"]]}),
	)
	.await;
	assert_eq!(status, 200, "{sent}");
	assert_eq!(f.store.run_inputs(run.id).await.unwrap().len(), 1);
}

async fn verify_expired_route_retry(
	app: &common::TestApplication,
	f: &Federation,
	worker: &Harness,
	operator: &str,
) {
	let mut model = f.registry.get("model", "1.0.1").await.unwrap();
	model.version = "1.0.2".into();
	model.config["media_routes"][0]["expires_at"] =
		json!(chrono::Utc::now() + chrono::Duration::seconds(3));
	let (status, body) = request(app, operator, "POST", "/api/registry", json!(model)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		operator,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"model","version":"1.0.2"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut agent = f.registry.get("research", "1.0.1").await.unwrap();
	agent.version = "1.0.2".into();
	agent.config["model"]["version"] = json!("1.0.2");
	let (status, body) = request(app, operator, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		operator,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":"1.0.2"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, created) = request(
		app,
		operator,
		"POST",
		"/api/conversations",
		json!({"title":"Retry after route expiry", "goal":"Inspect media", "target":{"id":"research","version":"1.0.2"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let workspace = created["workspace"]["id"].as_str().unwrap();
	let run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == workspace)
		.unwrap();
	let image = upload(
		app,
		operator,
		workspace,
		"retry.png",
		"image/png",
		b"\x89PNG\r\n\x1a\nretry",
	)
	.await;
	let key = Uuid::new_v4();
	let input = json!({"content":"retry", "idempotency_key":key, "attachment_ids":[image["id"]]});
	let path = format!("/api/runs/{}/message", run.id);
	let (status, body) = request(app, operator, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{body}");
	tokio::time::sleep(std::time::Duration::from_secs(4)).await;
	let (status, body) = request(app, operator, "POST", &path, input).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(f.store.run_inputs(run.id).await.unwrap().len(), 1);
	assert!(worker.worker_once().await.unwrap());
	assert!(worker.worker_once().await.unwrap());
	let paused = f.store.run(run.id).await.unwrap();
	assert_eq!(paused.control.as_str(), "PAUSED");
	assert_eq!(paused.phase().as_str(), "THINKING");
	assert_eq!(paused.context.media_inferred_seq, 0);
	assert_eq!(f.store.run_inputs(run.id).await.unwrap().len(), 1);
}

async fn verify_separate_format_routes(
	app: &common::TestApplication,
	f: &Federation,
	worker: &Harness,
	received: &mut mpsc::UnboundedReceiver<Value>,
	operator: &str,
) {
	let mut model = f.registry.get("model", "1.0.1").await.unwrap();
	model.version = "1.0.3".into();
	model.config["media_routes"] = json!([
		{"tag":"fixture/png", "formats":["image/png"], "source":"mock provider contract",
		 "verified_at":chrono::Utc::now() - chrono::Duration::hours(1),
		 "expires_at":chrono::Utc::now() + chrono::Duration::hours(1)},
		{"tag":"fixture/jpeg", "formats":["image/jpeg"], "source":"mock provider contract",
		 "verified_at":chrono::Utc::now() - chrono::Duration::hours(1),
		 "expires_at":chrono::Utc::now() + chrono::Duration::hours(1)}
	]);
	let (status, body) = request(app, operator, "POST", "/api/registry", json!(model)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		operator,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"model","version":"1.0.3"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut agent = f.registry.get("research", "1.0.1").await.unwrap();
	agent.version = "1.0.3".into();
	agent.config["model"]["version"] = json!("1.0.3");
	let (status, body) = request(app, operator, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		operator,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":"1.0.3"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, created) = request(
		app,
		operator,
		"POST",
		"/api/conversations",
		json!({"title":"Separate media routes", "goal":"Inspect both images", "target":{"id":"research","version":"1.0.3"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let workspace = created["workspace"]["id"].as_str().unwrap();
	let run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == workspace)
		.unwrap();
	let images = [
		(
			"first.png",
			"image/png",
			b"\x89PNG\r\n\x1a\nfirst".as_slice(),
			"fixture/png",
		),
		(
			"second.jpg",
			"image/jpeg",
			b"\xff\xd8\xffsecond".as_slice(),
			"fixture/jpeg",
		),
	];
	for (name, mime, bytes, _) in images {
		let attachment = upload(app, operator, workspace, name, mime, bytes).await;
		let (status, sent) = request(
			app,
			operator,
			"POST",
			&format!("/api/runs/{}/message", run.id),
			json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":[attachment["id"]]}),
		)
		.await;
		assert_eq!(status, 200, "{sent}");
	}
	let mut seen = Vec::new();
	for _ in 0..12 {
		assert!(worker.worker_once().await.unwrap());
		while let Ok(body) = received.try_recv() {
			let Some(parts) = body["messages"][1]["content"].as_array() else {
				continue;
			};
			let urls: Vec<_> = parts
				.iter()
				.filter_map(|part| part["image_url"]["url"].as_str())
				.collect();
			if urls.is_empty() {
				continue;
			}
			assert_eq!(urls.len(), 1, "each request needs one common media route");
			for (index, (_, mime, bytes, tag)) in images.iter().enumerate() {
				let expected = format!(
					"data:{mime};base64,{}",
					base64::engine::general_purpose::STANDARD.encode(bytes)
				);
				if urls[0] == expected {
					assert_eq!(body["provider"]["only"], json!([tag]));
					if !seen.contains(&index) {
						seen.push(index);
					}
				}
			}
		}
		if seen.len() == 2 {
			break;
		}
	}
	assert_eq!(seen, vec![0, 1], "both formats must reach their own route");
	for _ in 0..4 {
		if f.store.run(run.id).await.unwrap().phase().as_str() == "COMPLETED" {
			break;
		}
		assert!(worker.worker_once().await.unwrap());
	}
	assert_eq!(
		f.store.run(run.id).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn human_media_only_run_input_reaches_the_first_model_request_in_order(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	media_requests: MediaRequests,
	#[from(empty_media_observation)] _empty_media: Arc<AtomicBool>,
	#[from(media_router)]
	#[with(media_requests.clone(), _empty_media.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) {
	let mut received = media_requests.receiver.lock().unwrap().take().unwrap();

	let endpoint = server.url.clone();
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, _) = bootstrap(&f, &app, &endpoint).await;
	policy["subjects"]
		[aidash_server::domain::qualified_agent(&f.config.node_id, "research", "1.0.1")] =
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
	model.config["context_window"] = json!(64_000);
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
	let (status, details) = request(
		&app,
		&token,
		"GET",
		&format!("/api/runs/{}", run.id),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{details}");
	assert!(
		details["media_input_routes"][0]
			.as_array()
			.unwrap()
			.contains(&json!("image/png"))
	);
	assert!(
		details["media_input_routes"][0]
			.as_array()
			.unwrap()
			.contains(&json!("audio/wav"))
	);
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
	let mut large_audio = vec![0_u8; 512 * 1024];
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
	assert_eq!(
		f.store.run(run.id).await.unwrap().phase().as_str(),
		"TOOL_CALL"
	);
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
	assert_eq!(
		f.store.run(run.id).await.unwrap().phase().as_str(),
		"THINKING"
	);
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
		if f.store.run(run.id).await.unwrap().phase().as_str() == "COMPLETED" {
			break;
		}
		assert!(worker.worker_once().await.unwrap());
	}
	assert_eq!(
		f.store.run(run.id).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
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
	let mut image_ids = Vec::new();
	let mut expected_urls = Vec::new();
	for index in 0..10_u8 {
		let mut bytes = b"\x89PNG\r\n\x1a\noperator".to_vec();
		bytes.push(b'0' + index);
		let uploaded = upload(
			&app,
			&operator,
			operator_workspace,
			&format!("operator-{index}.png"),
			"image/png",
			&bytes,
		)
		.await;
		image_ids.push(uploaded["id"].as_str().unwrap().to_owned());
		expected_urls.push(format!(
			"data:image/png;base64,{}",
			base64::engine::general_purpose::STANDARD.encode(&bytes)
		));
	}
	let original_inputs = f.store.run_inputs(operator_run.id).await.unwrap().len();
	let (status, rejected) = request(
		&app,
		&operator,
		"POST",
		&format!("/api/runs/{}/message", operator_run.id),
		json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":&image_ids[..9]}),
	)
	.await;
	assert_eq!(status, 400, "{rejected}");
	assert_eq!(
		f.store.run_inputs(operator_run.id).await.unwrap().len(),
		original_inputs
	);
	for ids in image_ids.chunks(5) {
		let (status, sent) = request(
			&app,
			&operator,
			"POST",
			&format!("/api/runs/{}/message", operator_run.id),
			json!({"content":"","idempotency_key":Uuid::new_v4(),"attachment_ids":ids}),
		)
		.await;
		assert_eq!(status, 200, "{sent}");
	}
	let mut media_batches = Vec::new();
	for _ in 0..12 {
		assert!(worker.worker_once().await.unwrap());
		while let Ok(body) = received.try_recv() {
			let Some(parts) = body["messages"][1]["content"].as_array() else {
				continue;
			};
			let urls: Vec<String> = parts
				.iter()
				.filter_map(|part| part["image_url"]["url"].as_str().map(str::to_owned))
				.collect();
			if !urls.is_empty() {
				assert_eq!(urls.len(), 5);
				assert_eq!(body["provider"]["only"], json!(["fixture/verified"]));
				if media_batches.len() < 2 {
					assert!(body.get("tools").is_none());
				}
				media_batches.push(urls);
			}
		}
		if media_batches.len() == 3 {
			break;
		}
	}
	let final_operator_run = f.store.run(operator_run.id).await.unwrap();
	assert!(
		media_batches.len() == 3,
		"operator run did not retry the empty observation and infer both accepted media batches: {} {} {:?} {:?} lease={:?} until={:?} task={:?}",
		final_operator_run.phase().as_str(),
		final_operator_run.control,
		final_operator_run.error,
		final_operator_run.state,
		final_operator_run.lease_owner,
		final_operator_run.lease_until,
		f.store
			.task(final_operator_run.task_id)
			.await
			.unwrap()
			.status
	);
	assert_eq!(media_batches[0], media_batches[1]);
	assert_eq!(
		[media_batches[1].clone(), media_batches[2].clone()].concat(),
		expected_urls
	);
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(
		f.store.run(operator_run.id).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
	Box::pin(verify_audio_batches(
		&app,
		&f,
		&worker,
		&mut received,
		&operator,
		AudioBatchCase {
			first_audio: &large_audio,
			message_text: "",
			leading_text: None,
		},
	))
	.await;
	let text = "a".repeat(5_500);
	Box::pin(verify_audio_batches(
		&app,
		&f,
		&worker,
		&mut received,
		&operator,
		AudioBatchCase {
			first_audio: &large_audio[..380 * 1024],
			message_text: &text,
			leading_text: None,
		},
	))
	.await;
	let mut near_window_audio = vec![0_u8; 700 * 1024];
	near_window_audio[..12].copy_from_slice(b"RIFF\0\0\0\0WAVE");
	let leading_text = "retain this instruction ".repeat(450);
	Box::pin(verify_audio_batches(
		&app,
		&f,
		&worker,
		&mut received,
		&operator,
		AudioBatchCase {
			first_audio: &near_window_audio,
			message_text: "",
			leading_text: Some(&leading_text),
		},
	))
	.await;
	Box::pin(verify_separate_format_routes(
		&app,
		&f,
		&worker,
		&mut received,
		&operator,
	))
	.await;
	Box::pin(verify_expired_route_retry(&app, &f, &worker, &operator)).await;
	Box::pin(verify_attachment_labels_count_toward_admission(
		&app, &f, &operator,
	))
	.await;
	drop(server);
	cleanup(f, &url, &schema).await;
}

#[derive(Clone)]
struct MediaRequests {
	sender: mpsc::UnboundedSender<Value>,
	receiver: Arc<std::sync::Mutex<Option<mpsc::UnboundedReceiver<Value>>>>,
}
#[rstest::fixture]
fn media_requests() -> MediaRequests {
	let (sender, receiver) = mpsc::unbounded_channel();
	MediaRequests {
		sender,
		receiver: Arc::new(std::sync::Mutex::new(Some(receiver))),
	}
}
#[rstest::fixture]
fn empty_media_observation() -> Arc<AtomicBool> {
	Arc::new(AtomicBool::new(true))
}
#[rstest::fixture]
fn media_router(
	media_requests: MediaRequests,
	empty_media_observation: Arc<AtomicBool>,
) -> Arc<Router> {
	let sent = media_requests.sender;
	Arc::new(Router::new()
		.handler("/v1/models/fixture/endpoints", handler(http::Method::GET, |_request: reinhardt::Request| async { reinhardt::Response::ok().with_json(&json!({"data":{"architecture":{"input_modalities":["text","image","audio"]},"endpoints":[{"tag":"fixture/verified","context_length":128000},{"tag":"fixture/png","context_length":128000},{"tag":"fixture/jpeg","context_length":128000}]}})).unwrap() }))
		.handler("/v1/endpoints/zdr", handler(http::Method::GET, |_request: reinhardt::Request| async { reinhardt::Response::ok().with_json(&json!({"data":[{"model_id":"fixture","tag":"fixture/verified"},{"model_id":"fixture","tag":"fixture/png"},{"model_id":"fixture","tag":"fixture/jpeg"}]})).unwrap() }))
		.handler("/v1/chat/completions", handler(http::Method::POST, move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
			let sent = sent.clone();
			let empty_media_observation = empty_media_observation.clone();
			async move {
				let image_count = body["messages"][1]["content"]
					.as_array()
					.map(|parts| parts.iter().filter(|part| part["type"] == "image_url").count())
					.unwrap_or(0);
				let no_tools = body.get("tools").is_none();
				sent.send(body).unwrap();
				if image_count == 5 && empty_media_observation.swap(false, Ordering::SeqCst) {
					assert!(no_tools, "media intake must not advertise task tools");
					return reinhardt::Response::ok().with_json(&json!({"choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{"id":"ignored-media-call","function":{"name":"workspace_read","arguments":"{}"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":2}})).unwrap();
				}
				reinhardt::Response::ok().with_json(&json!({"choices":[{"finish_reason":"stop","message":{"content":"Done"}}],"usage":{"prompt_tokens":10,"completion_tokens":2}})).unwrap()
			}
		})))
}
