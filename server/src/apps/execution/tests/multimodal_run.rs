#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_application::tenancy::TEST_FIXTURE;
use common::upstream_fixtures;
use reinhardt::ServerRouter as Router;
use reinhardt::test::fixtures::server::TestServerGuard;
use upstream_fixtures::{reply, upstream};
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

const PNG: &[u8] = include_bytes!("support/media/pixel.png");
const JPEG: &[u8] = include_bytes!("support/media/pixel.jpg");

fn wav_bytes(size: usize) -> Vec<u8> {
	let size = size.max(45);
	let mut bytes = vec![0_u8; size];
	bytes[..4].copy_from_slice(b"RIFF");
	bytes[4..8].copy_from_slice(&((size - 8) as u32).to_le_bytes());
	bytes[8..12].copy_from_slice(b"WAVE");
	bytes[12..16].copy_from_slice(b"fmt ");
	bytes[16..20].copy_from_slice(&16_u32.to_le_bytes());
	bytes[20..22].copy_from_slice(&1_u16.to_le_bytes());
	bytes[22..24].copy_from_slice(&1_u16.to_le_bytes());
	bytes[24..28].copy_from_slice(&48_000_u32.to_le_bytes());
	bytes[28..32].copy_from_slice(&48_000_u32.to_le_bytes());
	bytes[32..34].copy_from_slice(&1_u16.to_le_bytes());
	bytes[34..36].copy_from_slice(&8_u16.to_le_bytes());
	bytes[36..40].copy_from_slice(b"data");
	bytes[40..44].copy_from_slice(&((size - 44) as u32).to_le_bytes());
	bytes
}

async fn upload_response(
	app: &common::TestApplication,
	token: &str,
	workspace: &str,
	name: &str,
	mime: &str,
	bytes: &[u8],
) -> (u16, Value) {
	let path = format!(
		"/api/workspaces/{workspace}/attachments?filename={name}&media_type={}&idempotency_key={}",
		mime.replace('/', "%2F"),
		Uuid::new_v4()
	);

	let authorization = format!("Bearer {token}");
	let response = async {
		let client = &(app.api_http);
		let mut request = client
			.request(http::Method::POST, &path)
			.body(bytes::Bytes::copy_from_slice(bytes))
			.header(http::header::CONTENT_TYPE, "application/octet-stream");
		request = request.header("authorization", authorization.as_str());
		request.send().await
	}
	.await
	.unwrap();
	let status = response.status_code();
	let body = response.json_value().unwrap();
	(status, body)
}

async fn upload(
	app: &common::TestApplication,
	token: &str,
	workspace: &str,
	name: &str,
	mime: &str,
	bytes: &[u8],
) -> Value {
	let (status, body) = upload_response(app, token, workspace, name, mime, bytes).await;
	assert_eq!(status, 200, "{body}");
	body
}

struct AudioBatchCase<'a> {
	first_audio: &'a [u8],
	with_text: bool,
	leading_text: Option<&'a str>,
}

async fn verify_audio_batches(
	app: &common::TestApplication,
	f: &Federation,
	worker: &Harness,
	received: &mut mpsc::UnboundedReceiver<Value>,
	member: &str,
	case: AudioBatchCase<'_>,
) {
	let (status, audio_created) = request(
		app,
		member,
		"POST",
		"/api/conversations",
		json!({"title":"Audio batching", "goal":"Inspect both clips", "target":{"id":"research","version":"1.0.1"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{audio_created}");
	let audio_workspace = audio_created["workspace"]["id"].as_str().unwrap();
	let audio_run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == audio_workspace)
		.unwrap();
	// Both messages must fit the admitted Binding's ledger budget. The audio
	// still forces separate provider requests; tool declarations determine the
	// remaining window, so a fixed text length is not a stable fixture boundary.
	let message_limit = f.run_message_limit(&audio_run).await.unwrap();
	let message_text = if case.with_text {
		"a".repeat(message_limit / 3)
	} else {
		String::new()
	};
	let leading_text = case
		.leading_text
		.map(|text| text.chars().take(message_limit / 2).collect::<String>());
	// Reserve space for media framing and intake context while keeping this
	// clip near the actual window. The leading instruction then needs its own
	// request, even when Binding declarations reduce the available headroom.
	let audio_limit = message_limit
		.saturating_mul(4)
		.saturating_sub(3_072)
		.saturating_mul(16);
	let first_audio = wav_bytes(case.first_audio.len().min(audio_limit));
	if let Some(content) = &leading_text {
		let (status, sent) = request(
			app,
			member,
			"POST",
			&format!("/api/runs/{}/message", audio_run.id),
			json!({"content":content,"idempotency_key":Uuid::new_v4()}),
		)
		.await;
		assert_eq!(status, 200, "{sent}");
	}
	let mut second_audio = first_audio.clone();
	*second_audio.last_mut().unwrap() = 1;
	let first = upload(
		app,
		member,
		audio_workspace,
		"first.wav",
		"audio/wav",
		&first_audio,
	)
	.await;
	let second = upload(
		app,
		member,
		audio_workspace,
		"second.wav",
		"audio/wav",
		&second_audio,
	)
	.await;
	for attachment in [first, second] {
		let (status, sent) = request(
			app,
			member,
			"POST",
			&format!("/api/runs/{}/message", audio_run.id),
			json!({"content":message_text, "idempotency_key":Uuid::new_v4(), "attachment_ids":[attachment["id"]]}),
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
				if let Some(prefix) = &leading_text
					&& body["messages"][1]["content"]
						.as_str()
						.is_some_and(|content| content.contains(prefix.as_str()))
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
				if case.with_text {
					assert!(parts.iter().any(|part| {
						part["text"]
							.as_str()
							.is_some_and(|text| text.contains(&message_text))
					}));
				}
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
			base64::engine::general_purpose::STANDARD.encode(&first_audio),
			base64::engine::general_purpose::STANDARD.encode(&second_audio),
		]
	);
	if leading_text.is_some() {
		assert!(
			saw_prefix_batch,
			"the leading text must be deferred separately"
		);
	}
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, audio_run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"COMPLETED"
	);
}

async fn verify_attachment_labels_count_toward_admission(
	app: &common::TestApplication,
	f: &Federation,
	member: &str,
) {
	let (status, created) = request(
		app,
		member,
		"POST",
		"/api/conversations",
		json!({"title":"Media label budget", "goal":"Inspect audio", "target":{"id":"research","version":"1.0.1"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let workspace = created["workspace"]["id"].as_str().unwrap();
	let run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == workspace)
		.unwrap();
	let seq = f
		.store
		.run_inputs(&TEST_FIXTURE, run.id)
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
	let audio = wav_bytes(audio_tokens * 16);
	let long = upload(app, member, workspace, &long_name, "audio/wav", &audio).await;
	let short = upload(app, member, workspace, short_name, "audio/wav", &audio).await;
	let path = format!("/api/runs/{}/message", run.id);
	let (status, rejected) = request(
		app,
		member,
		"POST",
		&path,
		json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":[long["id"]]}),
	)
	.await;
	assert_eq!(status, 400, "{rejected}");
	assert!(
		f.store
			.run_inputs(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.is_empty()
	);
	let (status, sent) = request(
		app,
		member,
		"POST",
		&path,
		json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":[short["id"]]}),
	)
	.await;
	assert_eq!(status, 200, "{sent}");
	assert_eq!(
		f.store
			.run_inputs(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.len(),
		1
	);
}

async fn verify_expired_route_retry(
	app: &common::TestApplication,
	f: &Federation,
	worker: &Harness,
	member: &str,
) {
	let mut model = f
		.registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"model",
			"1.0.1",
		)
		.await
		.unwrap();
	model.version = "1.0.2".into();
	model.config["media_routes"][0]["expires_at"] =
		json!(chrono::Utc::now() + chrono::Duration::seconds(3));
	let (status, body) = request(app, member, "POST", "/api/registry", json!(model)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		member,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"model","version":"1.0.2"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut agent = f
		.registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"research",
			"1.0.1",
		)
		.await
		.unwrap();
	agent.version = "1.0.2".into();
	agent.binding_normalization = None;
	agent.config["model"]["version"] = json!("1.0.2");
	let (status, body) = request(app, member, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		member,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.0.2"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, created) = request(
		app,
		member,
		"POST",
		"/api/conversations",
		json!({"title":"Retry after route expiry", "goal":"Inspect media", "target":{"id":"research","version":"1.0.2"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let workspace = created["workspace"]["id"].as_str().unwrap();
	let run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == workspace)
		.unwrap();
	let image = upload(app, member, workspace, "retry.png", "image/png", PNG).await;
	let key = Uuid::new_v4();
	let input = json!({"content":"retry", "idempotency_key":key, "attachment_ids":[image["id"]]});
	let path = format!("/api/runs/{}/message", run.id);
	let (status, body) = request(app, member, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{body}");
	tokio::time::sleep(std::time::Duration::from_secs(4)).await;
	let (status, body) = request(app, member, "POST", &path, input).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		f.store
			.run_inputs(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.len(),
		1
	);
	assert!(worker.worker_once().await.unwrap());
	assert!(worker.worker_once().await.unwrap());
	let paused = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(paused.control.as_str(), "PAUSED");
	assert_eq!(paused.phase().as_str(), "THINKING");
	assert_eq!(paused.context.media_inferred_seq, 0);
	assert_eq!(
		f.store
			.run_inputs(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.len(),
		1
	);
}

async fn verify_separate_format_routes(
	app: &common::TestApplication,
	f: &Federation,
	worker: &Harness,
	received: &mut mpsc::UnboundedReceiver<Value>,
	member: &str,
) {
	let mut model = f
		.registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"model",
			"1.0.1",
		)
		.await
		.unwrap();
	model.version = "1.0.3".into();
	model.config["media_routes"] = json!([
		{"tag":"fixture/png", "formats":["image/png"], "source":"mock provider contract",
		 "verified_at":chrono::Utc::now() - chrono::Duration::hours(1),
		 "expires_at":chrono::Utc::now() + chrono::Duration::hours(1)},
		{"tag":"fixture/jpeg", "formats":["image/jpeg"], "source":"mock provider contract",
		 "verified_at":chrono::Utc::now() - chrono::Duration::hours(1),
		 "expires_at":chrono::Utc::now() + chrono::Duration::hours(1)}
	]);
	let (status, body) = request(app, member, "POST", "/api/registry", json!(model)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		member,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"model","version":"1.0.3"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut agent = f
		.registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"research",
			"1.0.1",
		)
		.await
		.unwrap();
	agent.version = "1.0.3".into();
	agent.binding_normalization = None;
	agent.config["model"]["version"] = json!("1.0.3");
	let (status, body) = request(app, member, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		app,
		member,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.0.3"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, created) = request(
		app,
		member,
		"POST",
		"/api/conversations",
		json!({"title":"Separate media routes", "goal":"Inspect both images", "target":{"id":"research","version":"1.0.3"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let workspace = created["workspace"]["id"].as_str().unwrap();
	let run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == workspace)
		.unwrap();
	let images = [
		("first.png", "image/png", PNG, "fixture/png"),
		("second.jpg", "image/jpeg", JPEG, "fixture/jpeg"),
	];
	for (name, mime, bytes, _) in images {
		let attachment = upload(app, member, workspace, name, mime, bytes).await;
		let (status, sent) = request(
			app,
			member,
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
		if f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str() == "COMPLETED"
		{
			break;
		}
		assert!(worker.worker_once().await.unwrap());
	}
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
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
	policy["subjects"][aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(f.config.deployment.as_str(), "acme"),
		"research",
		"1.0.1",
	)] = json!({"kind":"agent"});
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut model = f
		.registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"model",
			"1.0.0",
		)
		.await
		.unwrap();
	model.version = "1.0.1".into();
	model.config["context_window"] = json!(64_000);
	model.config["modalities"] = json!(["text", "image", "audio"]);
	model.config["media_routes"] = json!([{
		"tag":"fixture/verified", "formats":["image/png", "wav"],
		"source":"mock provider contract", "verified_at": chrono::Utc::now() - chrono::Duration::hours(1),
		"expires_at": chrono::Utc::now() + chrono::Duration::hours(1)
	}]);
	let (status, body) = request(&app, &token, "POST", "/api/registry", json!(model)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		&app,
		&token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"model","version":"1.0.1"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut agent = f
		.registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"research",
			"1.0.0",
		)
		.await
		.unwrap();
	agent.version = "1.0.1".into();
	agent.binding_normalization = None;
	agent.config["model"]["version"] = json!("1.0.1");
	let (status, body) = request(&app, &token, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(
		&app,
		&token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.0.1"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({"title":"Media input","goal":"Inspect media","target":{"id":"research","version":"1.0.1"},"target_kind":"agent"})).await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
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
	assert!(
		details["media_input_routes"][0]
			.as_array()
			.unwrap()
			.contains(&json!("audio/mpeg"))
	);
	let (status, rejected) = upload_response(
		&app,
		&token,
		&workspace,
		"bad.png",
		"image/png",
		b"not a png",
	)
	.await;
	assert_eq!(status, 400, "{rejected}");
	assert_eq!(rejected["error"], "MEDIA_UNDECODABLE");
	let path = format!("/api/runs/{}/message", run.id);
	assert!(
		f.store
			.run_inputs(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.is_empty()
	);
	// The route accepts WAV, but this MP3 would expand beyond the inference budget.
	let long_audio = upload(
		&app,
		&token,
		&workspace,
		"long.mp3",
		"audio/mpeg",
		include_bytes!("support/media/too-long-for-wav.mp3"),
	)
	.await;
	let long_id: Uuid = long_audio["id"].as_str().unwrap().parse().unwrap();
	let (status, refused) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":[long_id]}),
	)
	.await;
	assert_eq!(status, 400, "{refused}");
	assert!(
		refused["error"]
			.as_str()
			.unwrap()
			.contains("too long to convert"),
		"{refused}"
	);
	let (cached,): (Option<Uuid>,) =
		sqlx::query_as("SELECT wav_object_id FROM channel_attachments WHERE id = $1")
			.bind(long_id)
			.fetch_one(f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(cached.is_none());
	let large_audio = wav_bytes(512 * 1024);
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
	assert!(
		f.store
			.run_inputs(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.is_empty()
	);
	let audio = upload(
		&app,
		&token,
		&workspace,
		"sample.wav",
		"audio/wav",
		&wav_bytes(100),
	)
	.await;
	let image = upload(&app, &token, &workspace, "sample.png", "image/png", PNG).await;
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
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
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
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
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
		if f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str() == "COMPLETED"
		{
			break;
		}
		assert!(worker.worker_once().await.unwrap());
	}
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"COMPLETED"
	);
	let member = token.clone();
	let (status, created) = request(
		&app,
		&member,
		"POST",
		"/api/conversations",
		json!({"title":"Subject media", "goal":"Inspect image", "target":{"id":"research","version":"1.0.1"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let member_workspace = created["workspace"]["id"].as_str().unwrap();
	let member_run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == member_workspace)
		.unwrap();
	let mut image_ids = Vec::new();
	let mut expected_urls = Vec::new();
	for index in 0..10_u8 {
		let mut bytes = PNG.to_vec();
		bytes.push(b'0' + index);
		let uploaded = upload(
			&app,
			&member,
			member_workspace,
			&format!("member-{index}.png"),
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
	let original_inputs = f
		.store
		.run_inputs(&TEST_FIXTURE, member_run.id)
		.await
		.unwrap()
		.len();
	let (status, rejected) = request(
		&app,
		&member,
		"POST",
		&format!("/api/runs/{}/message", member_run.id),
		json!({"content":"", "idempotency_key":Uuid::new_v4(), "attachment_ids":&image_ids[..9]}),
	)
	.await;
	assert_eq!(status, 400, "{rejected}");
	assert_eq!(
		f.store
			.run_inputs(&TEST_FIXTURE, member_run.id)
			.await
			.unwrap()
			.len(),
		original_inputs
	);
	for ids in image_ids.chunks(5) {
		let (status, sent) = request(
			&app,
			&member,
			"POST",
			&format!("/api/runs/{}/message", member_run.id),
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
	let final_member_run = f.store.run(&TEST_FIXTURE, member_run.id).await.unwrap();
	assert!(
		media_batches.len() == 3,
		"subject run did not retry the empty observation and infer both accepted media batches: {} {} {:?} {:?} lease={:?} until={:?} task={:?}",
		final_member_run.phase().as_str(),
		final_member_run.control,
		final_member_run.error,
		final_member_run.state,
		final_member_run.lease_owner,
		final_member_run.lease_until,
		f.store
			.task(&TEST_FIXTURE, final_member_run.task_id)
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
		f.store
			.run(&TEST_FIXTURE, member_run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"COMPLETED"
	);
	Box::pin(verify_audio_batches(
		&app,
		&f,
		&worker,
		&mut received,
		&member,
		AudioBatchCase {
			first_audio: &large_audio,
			with_text: false,
			leading_text: None,
		},
	))
	.await;
	Box::pin(verify_audio_batches(
		&app,
		&f,
		&worker,
		&mut received,
		&member,
		AudioBatchCase {
			first_audio: &large_audio[..380 * 1024],
			with_text: true,
			leading_text: None,
		},
	))
	.await;
	let near_window_audio = wav_bytes(700 * 1024);
	let leading_text = "retain this instruction ".repeat(450);
	Box::pin(verify_audio_batches(
		&app,
		&f,
		&worker,
		&mut received,
		&member,
		AudioBatchCase {
			first_audio: &near_window_audio,
			with_text: false,
			leading_text: Some(&leading_text),
		},
	))
	.await;
	Box::pin(verify_separate_format_routes(
		&app,
		&f,
		&worker,
		&mut received,
		&member,
	))
	.await;
	Box::pin(verify_expired_route_retry(&app, &f, &worker, &member)).await;
	Box::pin(verify_attachment_labels_count_toward_admission(
		&app, &f, &member,
	))
	.await;
	// A route which only accepts WAV gets one immutable cached rendition of an MP3.
	let (status, created) = request(&app, &member, "POST", "/api/conversations", json!({"title":"Audio rendition", "goal":"Inspect audio", "target":{"id":"research","version":"1.0.1"},"target_kind":"agent"})).await;
	assert_eq!(status, 200, "{created}");
	let workspace = created["workspace"]["id"].as_str().unwrap();
	let run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == workspace)
		.unwrap();
	let source = include_bytes!("support/media/short.mp3");
	let attachment = upload(&app, &member, workspace, "sample.mp3", "audio/mpeg", source).await;
	let attachment_id: Uuid = attachment["id"].as_str().unwrap().parse().unwrap();
	let key = Uuid::new_v4();
	let path = format!("/api/runs/{}/message", run.id);
	let input = json!({"content":"", "idempotency_key":key, "attachment_ids":[attachment_id]});
	let (status, body) = request(&app, &member, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{body}");
	let cached: (Uuid, Uuid, String, i64) = sqlx::query_as("SELECT object_id, wav_object_id, wav_sha256, wav_size_bytes FROM channel_attachments WHERE id = $1")
		.bind(attachment_id).fetch_one(f.store.pool.fixture_driver()).await.unwrap();
	assert_ne!(cached.0, cached.1);
	assert!(cached.3 > 44);
	let storage = &f.store.capabilities.0.storage;
	assert_eq!(
		tokio::fs::read(storage.join(cached.0.simple().to_string()))
			.await
			.unwrap(),
		source
	);
	let wav = tokio::fs::read(storage.join(cached.1.simple().to_string()))
		.await
		.unwrap();
	assert_eq!(&wav[..4], b"RIFF");
	assert_eq!(&wav[8..12], b"WAVE");
	assert_eq!(wav.len() as i64, cached.3);
	assert_eq!(request(&app, &member, "POST", &path, input).await.0, 200);
	while received.try_recv().is_ok() {}
	let mut saw_rendition = false;
	for _ in 0..8 {
		assert!(worker.worker_once().await.unwrap());
		if let Ok(body) = received.try_recv() {
			let parts = body["messages"][1]["content"].as_array().unwrap();
			assert!(
				parts
					.iter()
					.any(|part| part["input_audio"]["format"] == "wav"),
				"{parts:?}"
			);
			saw_rendition = true;
			break;
		}
	}
	assert!(saw_rendition, "model did not receive the WAV rendition");
	let again: (Uuid, Uuid, String, i64) = sqlx::query_as("SELECT object_id, wav_object_id, wav_sha256, wav_size_bytes FROM channel_attachments WHERE id = $1")
		.bind(attachment_id).fetch_one(f.store.pool.fixture_driver()).await.unwrap();
	assert_eq!(again, cached);
	// Removing the attachment releases both immutable objects through reclamation.
	sqlx::query("DELETE FROM channel_attachments WHERE id = $1")
		.bind(attachment_id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (stop, rx) = tokio::sync::watch::channel(false);
	let reclaim = tokio::spawn(aidash_server::capabilities::reclamation::run(
		f.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
	loop {
		let (count,): (i64,) =
			sqlx::query_as("SELECT count(*) FROM core_objects WHERE id = $1 OR id = $2")
				.bind(cached.0)
				.bind(cached.1)
				.fetch_one(f.store.pool.fixture_driver())
				.await
				.unwrap();
		if count == 0 {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"attachment objects were not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	stop.send(true).unwrap();
	reclaim.await.unwrap().unwrap();
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
	Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/models/fixture/endpoints", http::Method::GET, reply(|_request: reinhardt::Request| async { reinhardt::Response::ok().with_json(&json!({"data":{"architecture":{"input_modalities":["text","image","audio"]},"endpoints":[{"tag":"fixture/verified","context_length":128000},{"tag":"fixture/png","context_length":128000},{"tag":"fixture/jpeg","context_length":128000}]}})).unwrap() }))
.route("/v1/endpoints/zdr", http::Method::GET, reply(|_request: reinhardt::Request| async { reinhardt::Response::ok().with_json(&json!({"data":[{"model_id":"fixture","tag":"fixture/verified"},{"model_id":"fixture","tag":"fixture/png"},{"model_id":"fixture","tag":"fixture/jpeg"}]})).unwrap() }))
.route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
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
		})).into_server_router())
}

#[derive(Clone)]
struct SelectedThreadRequests {
	sender: mpsc::UnboundedSender<Value>,
	receiver: Arc<std::sync::Mutex<Option<mpsc::UnboundedReceiver<Value>>>>,
	attachment: Arc<std::sync::Mutex<Option<String>>>,
	revision: Arc<std::sync::Mutex<i64>>,
}
#[rstest::fixture]
fn selected_thread_requests() -> SelectedThreadRequests {
	let (sender, receiver) = mpsc::unbounded_channel();
	SelectedThreadRequests {
		sender,
		receiver: Arc::new(std::sync::Mutex::new(Some(receiver))),
		attachment: Arc::new(std::sync::Mutex::new(None)),
		revision: Arc::new(std::sync::Mutex::new(0)),
	}
}
#[rstest::fixture]
fn selected_thread_router(
	#[from(selected_thread_requests)] requests: SelectedThreadRequests,
) -> Arc<Router> {
	let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
	Arc::new(reinhardt::test::stub::StubRouter::new()
	.route("/v1/models/fixture/endpoints", http::Method::GET, reply(|_request: reinhardt::Request| async { reinhardt::Response::ok().with_json(&json!({"data":{"architecture":{"input_modalities":["text","image"]},"endpoints":[{"tag":"fixture/verified","context_length":128000}]}})).unwrap() }))
	.route("/v1/endpoints/zdr", http::Method::GET, reply(|_request: reinhardt::Request| async { reinhardt::Response::ok().with_json(&json!({"data":[{"model_id":"fixture","tag":"fixture/verified"}]})).unwrap() }))
	.route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {
		let body = request.json::<Value>().unwrap();
		let requests = requests.clone();
		let count = count.clone();
		async move {
			requests.sender.send(body).unwrap();
			let step = count.fetch_add(1, Ordering::SeqCst);
			let call = match step {
				0 => Some(("workspace_observe", json!({}))),
				1 | 6 | 7 | 9 => Some(("workspace_read", json!({"kind":"attachment", "id":requests.attachment.lock().unwrap().as_ref().unwrap(), "representation":"model_input"}))),
				2 => Some(("attachment_copy", json!({"id":requests.attachment.lock().unwrap().as_ref().unwrap(), "path":"copied.png", "expected_revision":*requests.revision.lock().unwrap(), "idempotency_key":Uuid::new_v4()}))),
				3 => Some(("workspace_message", json!({"content":"Agent snapshot", "files":["copied.png"]}))),
				5 => Some(("workspace_observe", json!({}))),
				_ => None,
			};
			let message = match call {
				Some((name, arguments)) => json!({"content":if step == 2 { Some("The image is a pixel.") } else { None },"tool_calls":[{"id":format!("call-{step}"),"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}),
				None => json!({"content":"Done"}),
			};
			reinhardt::Response::ok().with_json(&json!({"choices":[{"finish_reason":if matches!(step, 0..=3 | 5..=7 | 9) {"tool_calls"} else {"stop"},"message":message}],"usage":{"prompt_tokens":10,"completion_tokens":2}})).unwrap()
		}
	})).into_server_router())
}

#[rstest::rstest]
#[tokio::test]
async fn visible_thread_attachment_is_selected_for_the_next_model_request(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	selected_thread_requests: SelectedThreadRequests,
	#[from(selected_thread_router)]
	#[with(selected_thread_requests.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) {
	let mut received = selected_thread_requests
		.receiver
		.lock()
		.unwrap()
		.take()
		.unwrap();
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (mut policy, token, _) = common::bootstrap_with_area(&f, &app, &server.url).await;
	policy["subjects"][aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(f.config.deployment.as_str(), "acme"),
		"research",
		"1.0.1",
	)] = json!({"kind":"agent"});
	policy["subjects"][aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(f.config.deployment.as_str(), "acme"),
		"research-peer",
		"1.0.1",
	)] = json!({"kind":"agent"});
	let (status, body) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut model = f.registry.get(&tenant, "model", "1.0.0").await.unwrap();
	model.version = "1.0.1".into();
	model.config["modalities"] = json!(["text", "image"]);
	model.config["media_routes"] = json!([{"tag":"fixture/verified", "formats":["image/png"], "source":"fixture", "verified_at":chrono::Utc::now() - chrono::Duration::hours(1), "expires_at":chrono::Utc::now() + chrono::Duration::hours(1)}]);
	let (status, body) = request(&app, &token, "POST", "/api/registry", json!(model)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&app, &token, "POST", "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"model","version":"1.0.1"},"expected_revision":0,"enabled":true})).await;
	assert_eq!(status, 200, "{body}");
	let mut agent = f.registry.get(&tenant, "research", "1.0.0").await.unwrap();
	agent.version = "1.0.1".into();
	agent.binding_normalization = None;
	agent.config["model"]["version"] = json!("1.0.1");
	let mut peer = agent.clone();
	peer.id = "research-peer".into();
	let (status, body) = request(&app, &token, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&app, &token, "POST", "/api/registry", json!(peer)).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&app, &token, "POST", "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research-peer","version":"1.0.1"},"expected_revision":0,"enabled":true})).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&app, &token, "POST", "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.0.1"},"expected_revision":0,"enabled":true})).await;
	assert_eq!(status, 200, "{body}");
	let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({"title":"Thread image", "goal":"Inspect the image", "target":{"id":"research","version":"1.0.1"}, "target_kind":"agent"})).await;
	assert_eq!(status, 200, "{created}");
	let run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.workspace_id.to_string() == created["workspace"]["id"].as_str().unwrap())
		.unwrap();
	let workspace = run.workspace_id.to_string();
	let (status, area) = request(
		&app,
		&token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	*selected_thread_requests.revision.lock().unwrap() = area["revision"].as_i64().unwrap();
	let attachment = upload(&app, &token, &workspace, "selected.png", "image/png", PNG).await;
	assert_eq!(
		attachment["no_assigned_agent_can_read"], false,
		"{attachment}"
	);
	let pdf = upload(
		&app,
		&token,
		&workspace,
		"notes.pdf",
		"application/pdf",
		b"test file",
	)
	.await;
	assert_eq!(pdf["no_assigned_agent_can_read"], false, "{pdf}");
	*selected_thread_requests.attachment.lock().unwrap() =
		Some(attachment["id"].as_str().unwrap().to_owned());
	let (status, body) = request(&app, &token, "POST", &format!("/api/workspaces/{workspace}/thread-messages"), json!({"content":"Please inspect", "idempotency_key":Uuid::new_v4(), "attachment_ids":[attachment["id"], pdf["id"]]})).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		body["attachments"][0]["no_assigned_agent_can_read"], false,
		"{body}"
	);
	// An unrelated corrupt attachment must not prevent copying the selected one.
	sqlx::query("UPDATE channel_attachments SET sha256 = $1 WHERE id = $2")
		.bind("0".repeat(64))
		.bind(Uuid::parse_str(pdf["id"].as_str().unwrap()).unwrap())
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	let worker = Harness {
		federation: f.clone(),
	};
	let mut requests = Vec::new();
	for _ in 0..30 {
		worker.worker_once().await.unwrap();
		while let Ok(body) = received.try_recv() {
			requests.push(body);
		}
		if requests.len() >= 5 {
			break;
		}
	}
	assert!(requests.len() >= 5, "provider requests: {requests:?}");
	let observation = requests[1].to_string();
	assert!(observation.contains("selected.png"), "{observation}");
	assert!(!observation.contains(&base64::engine::general_purpose::STANDARD.encode(PNG)));
	assert!(
		requests[2].to_string().contains("image_url"),
		"{}",
		requests[2]
	);
	let observed_run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|item| item.id == run.id)
		.unwrap();
	assert_eq!(observed_run.context.media_inferred_seq, 0);
	assert!(
		observed_run.context.history.iter().any(|entry| matches!(
			&entry.event,
			aidash_domain::context::ContextEvent::ModelMediaObservation {
				through_seq: None,
				text,
				..
			} if text == "The image is a pixel."
		)),
		"selected media observation was not recorded"
	);
	let (status, history) = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{workspace}/message-history"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{history}");
	assert_eq!(
		history["messages"]
			.as_array()
			.unwrap()
			.iter()
			.find(|item| item["message"]["content"] == "Please inspect")
			.unwrap()["attachments"][0]["no_assigned_agent_can_read"],
		false
	);
	let posted = history["messages"]
		.as_array()
		.unwrap()
		.iter()
		.find(|item| item["message"]["content"] == "Agent snapshot")
		.expect("Agent A posted its file snapshot");
	let posted_id = posted["attachments"][0]["id"].as_str().unwrap().to_owned();
	assert_ne!(posted_id, attachment["id"].as_str().unwrap());
	*selected_thread_requests.attachment.lock().unwrap() = Some(posted_id);
	let (status, task) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Peer", "description":"Inspect Agent A's image"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{}/delegate", task["id"].as_str().unwrap()),
		json!({"node_id":f.config.deployment, "agent":{"id":"research-peer", "version":"1.0.1"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	for _ in 0..30 {
		worker.worker_once().await.unwrap();
		while let Ok(body) = received.try_recv() {
			requests.push(body);
		}
		if requests.len() >= 8 {
			break;
		}
	}
	assert!(requests.len() >= 8, "provider requests: {requests:?}");
	assert!(requests[6].to_string().contains("Agent snapshot"));
	assert!(
		requests[7].to_string().contains("image_url"),
		"{}",
		requests[7]
	);
	assert!(
		requests[7]
			.to_string()
			.contains(&base64::engine::general_purpose::STANDARD.encode(PNG))
	);
	let peer_run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id.to_string() == task["id"].as_str().unwrap())
		.unwrap();
	let selected_id = selected_thread_requests
		.attachment
		.lock()
		.unwrap()
		.clone()
		.unwrap();
	let mut ready = false;
	for _ in 0..10 {
		worker.worker_once().await.unwrap();
		let current = f.store.run(&TEST_FIXTURE, peer_run.id).await.unwrap();
		if let aidash_server::domain::RunState::Thinking(state) = &current.state
			&& !state.selected_media.is_empty()
		{
			ready = true;
			break;
		}
	}
	assert!(ready, "Agent B did not retain its second selection");
	sqlx::query("DELETE FROM channel_attachments WHERE id = $1")
		.bind(Uuid::parse_str(&selected_id).unwrap())
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	for _ in 0..10 {
		worker.worker_once().await.unwrap();
		while let Ok(body) = received.try_recv() {
			requests.push(body);
		}
		if requests.len() >= 9 {
			break;
		}
	}
	assert!(requests.len() >= 9, "provider requests: {requests:?}");
	assert!(
		requests[8]
			.to_string()
			.contains("Thread Attachment unavailable"),
		"{}",
		requests[8]
	);
	*selected_thread_requests.attachment.lock().unwrap() =
		Some(attachment["id"].as_str().unwrap().to_owned());
	let (status, task) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Text-only", "description":"Try the image"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{}/delegate", task["id"].as_str().unwrap()),
		json!({"node_id":f.config.deployment, "agent":{"id":"research", "version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	for _ in 0..30 {
		worker.worker_once().await.unwrap();
		while let Ok(body) = received.try_recv() {
			requests.push(body);
		}
		if requests.len() >= 11 {
			break;
		}
	}
	assert!(requests.len() >= 11, "provider requests: {requests:?}");
	assert!(
		requests[10]
			.to_string()
			.contains("this model cannot read image/png through any current route"),
		"{}",
		requests[10]
	);
	let (status, text_conversation) = request(&app, &token, "POST", "/api/conversations", json!({"title":"Text only", "goal":"Inspect", "target":{"id":"research","version":"1.0.0"}, "target_kind":"agent"})).await;
	assert_eq!(status, 200, "{text_conversation}");
	let text_workspace = text_conversation["workspace"]["id"].as_str().unwrap();
	let text_image = upload(
		&app,
		&token,
		text_workspace,
		"unreadable.png",
		"image/png",
		PNG,
	)
	.await;
	assert_eq!(
		text_image["no_assigned_agent_can_read"], true,
		"{text_image}"
	);
	let (status, message) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{text_workspace}/thread-messages"),
		json!({"content":"Image", "idempotency_key":Uuid::new_v4(), "attachment_ids":[text_image["id"]]}),
	)
	.await;
	assert_eq!(status, 200, "{message}");
	assert_eq!(
		message["attachments"][0]["no_assigned_agent_can_read"],
		true
	);
	let (status, next_task) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{text_workspace}/tasks"),
		json!({"title":"Image reader", "description":"Inspect"}),
	)
	.await;
	assert_eq!(status, 200, "{next_task}");
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{}/delegate", next_task["id"].as_str().unwrap()),
		json!({"node_id":f.config.deployment, "agent":{"id":"research-peer", "version":"1.0.1"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let (status, listing) = request(
		&app,
		&token,
		"GET",
		&format!("/api/workspaces/{text_workspace}/message-history"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{listing}");
	let listed = listing["messages"]
		.as_array()
		.unwrap()
		.iter()
		.find(|entry| entry["message"]["content"] == "Image")
		.unwrap();
	assert_eq!(
		listed["attachments"][0]["no_assigned_agent_can_read"], false,
		"{listing}"
	);
	drop(server);
	cleanup(f, &url, &schema).await;
}
