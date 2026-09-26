//! A provider-driven journey: tool selection and tool arguments travel through
//! the real Harness, authorization guards, journal, and isolated runtime.
use super::*;
use axum::{Json, http::HeaderMap, routing::post};
use std::sync::{
	Mutex,
	atomic::{AtomicUsize, Ordering},
};

struct Journey {
	c: CoreFixture,
	peer: CoreFixture,
	receiver: aidash::domain::Run,
	peer_servers: Vec<tokio::task::JoinHandle<()>>,
	run: aidash::domain::Run,
	server: tokio::task::JoinHandle<()>,
	requests: Arc<Mutex<Vec<Value>>>,
	effects: Arc<AtomicUsize>,
}

fn next(context: &Value, recipient: &Value) -> Option<(&'static str, Value)> {
	let history = context["history"].as_array().unwrap();
	let results = |name: &str| {
		history
			.iter()
			.filter(|entry| entry["kind"] == "tool" && entry["call"]["name"] == name)
			.map(|entry| &entry["result"])
			.collect::<Vec<_>>()
	};
	let key = || Uuid::new_v4();
	let list = results("skill_list");
	if list.is_empty() {
		return Some(("skill_list", json!({})));
	}
	let skill = &list[0]["skills"][0];
	if results("skill_load").is_empty() {
		return Some((
			"skill_load",
			json!({"skill_id":skill["skill_id"],"expected_digest":skill["digest"]}),
		));
	}
	if results("skill_read").is_empty() {
		return Some((
			"skill_read",
			json!({"skill_id":skill["skill_id"],"digest":skill["digest"],"path":"references/guide.md"}),
		));
	}
	if results("apply_patch").is_empty() {
		return Some((
			"apply_patch",
			json!({"idempotency_key":key(),"expected_revision":1,"preconditions":{"data.csv":null},"patch":"*** Begin Patch\n*** Add File: data.csv\n+city,value\n+東京,41\n*** End Patch"}),
		));
	}
	let search = results("file_search");
	if search.is_empty() {
		return Some((
			"file_search",
			json!({"query":"東京","mode":"literal","scope":"working"}),
		));
	}
	if results("file_read").is_empty() {
		return Some((
			"file_read",
			json!({"file_id":search[0]["matches"][0]["file_id"],"representation":"text"}),
		));
	}
	let shells = results("shell");
	if shells.is_empty() {
		return Some((
			"shell",
			json!({"idempotency_key":key(),"expected_revision":2,"command":"python -c \"import os; from pathlib import Path; assert not any(k.startswith('AIDASH_') for k in os.environ); Path('shell.txt').write_text('once'); print('isolated shell')\""}),
		));
	}
	let shell_polls = results("shell_poll");
	if shell_polls
		.last()
		.is_none_or(|result| result["status"] != "completed")
	{
		return Some((
			"shell_poll",
			json!({"operation_id":shells[0]["operation_id"]}),
		));
	}
	let python = results("code_interpreter");
	if python.is_empty() {
		return Some((
			"code_interpreter",
			json!({"idempotency_key":key(),"expected_revision":shell_polls.last().unwrap()["revision"],"code":"import os\nassert not any(k.startswith('AIDASH_') for k in os.environ)\nimport pandas as pd\nframe = pd.read_csv('data.csv')\ncounter = int(frame['value'][0])\nassert counter == 41\nprint(counter)"}),
		));
	}
	let latest = python.last().unwrap();
	let polls = results("python_poll");
	let completed = polls
		.iter()
		.find(|p| p["operation_id"] == latest["operation_id"] && p["status"] == "completed");
	let Some(completed) = completed else {
		return Some((
			"python_poll",
			json!({"operation_id":latest["operation_id"]}),
		));
	};
	if python.len() == 1 {
		return Some((
			"code_interpreter",
			json!({"idempotency_key":key(),"expected_revision":completed["revision"],"expected_session_id":completed["session_id"],"code":"counter += 1\nassert counter == 42\nfrom pathlib import Path\nPath('answer.txt').write_text(str(counter))\nimport matplotlib.pyplot as plt\nplt.plot([41, counter])\nplt.show()\nprint(counter)"}),
		));
	}
	if results("plugin_0").is_empty() {
		return Some(("plugin_0", json!({"result":42})));
	}
	let shared = results("file_share");
	if shared
		.last()
		.is_none_or(|value| value["status"] != "completed")
	{
		let selected = results("file_search");
		if selected.len() == 1 {
			return Some((
				"file_search",
				json!({"query":"answer.txt","mode":"path","scope":"working"}),
			));
		}
		let file = &selected.last().unwrap()["matches"][0];
		// Reuse the exact prepared share input, including its request key.
		let input=history.iter().rev().find(|entry|entry["call"]["name"]=="file_share")
            .map(|entry|entry["call"]["arguments"].clone())
            .unwrap_or_else(||json!({"idempotency_key":key(),"expected_revision":completed["revision"],"files":[{"file_id":file["file_id"],"expected_digest":file["digest"]}],"recipient":recipient}));
		return Some(("file_share", input));
	}
	None
}

#[rstest::fixture]
async fn journey_fixture(#[future] test_environment: Arc<TestEnvironment>) -> Journey {
	let env = test_environment.await;
	let recipient = Arc::new(Mutex::new(Value::Null));
	let target = recipient.clone();
	let requests = Arc::new(Mutex::new(Vec::new()));
	let effects = Arc::new(AtomicUsize::new(0));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let capture = requests.clone();
	let counter = effects.clone();
	let server = Router::new().route("/v1/chat/completions", post(move |Json(body):Json<Value>| {
		let capture = capture.clone();
		let target = target.clone();
		async move {
			let context: Value = serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
			let count = { let mut requests = capture.lock().unwrap(); requests.push(body); requests.len() };
			let action = next(&context, &target.lock().unwrap());
			// Polling is observable work and consumes a model step; let the real
			// runner progress instead of exhausting the Agent budget in a busy loop.
			if action.as_ref().is_some_and(|(name,_)| name.ends_with("_poll") || *name == "file_share") { tokio::time::sleep(std::time::Duration::from_millis(300)).await; }
			let message = match action {
				Some((name, arguments)) => json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("journey-{count}"),"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}),
				None => json!({"role":"assistant","content":"Analyzed the CSV in an isolated persistent Python session. The verified answer is 42."}),
			};
			Json(json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
		}
	})).route("/effect", post(move |headers:HeaderMap, Json(_body):Json<Value>| {
		let counter = counter.clone();
		async move {
			assert_eq!(headers.get("authorization").unwrap(), &format!("Bearer {}", std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()));
			counter.fetch_add(1, Ordering::SeqCst);
			Json(json!({"saved":true}))
		}
	}));
	let server = tokio::spawn(async move {
		axum::serve(listener, server).await.unwrap();
	});
	let mut c = build_core_fixture_at(env.clone(), "aidash://journey", &endpoint).await;
	let mut profile = (*Runtime::from_env().unwrap().0).clone();
	profile.storage = c.root.clone();
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	let mut tool = c.f.registry.get("http", "1.0.0").await.unwrap();
	tool.version = "1.1.0".into();
	tool.config["credential_env"] = json!("AIDASH_SECRET_TEST_PEER");
	let mut agent = c.f.registry.get("research", "1.1.0").await.unwrap();
	agent.version = "1.2.0".into();
	agent.config["tools"] = json!([{"id":"http","version":"1.1.0"}]);
	agent.config["max_steps"] = json!(200);
	for entry in [tool, agent] {
		let (status, result) = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/registry",
			json!(entry),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		let (status, result) = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":entry.id,"version":entry.version},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
	}
	c.policy["subjects"]
		[aidash::domain::qualified_agent(&c.f.config.node_id, "research", "1.2.0")] =
		json!({"kind":"agent"});
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let mut peer = build_core_fixture(env, "aidash://journey-recipient").await;
	let (peer_servers, _) = super::transfer_tests::connect_nodes(&mut c, &mut peer, 3).await;
	let receiver = admit(&peer).await;
	*recipient.lock().unwrap() = json!({"node_id":peer.f.config.node_id,"agent_id":"research","agent_version":"1.1.0","thread_id":peer.task});
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", c.task),
		json!({"node_id":c.f.config.node_id,"agent":{"id":"research","version":"1.2.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let run = c.f.store.runs().await.unwrap().remove(0);
	Journey {
		c,
		peer,
		receiver,
		peer_servers,
		run,
		server,
		requests,
		effects,
	}
}

#[rstest::rstest]
#[tokio::test]
async fn harness_journey_keeps_core_names_state_and_integration_secrets_separate(
	#[future] journey_fixture: Journey,
) {
	let j = Box::pin(journey_fixture).await;
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let transfer = tokio::spawn(aidash::capabilities::transfer::run(
		j.c.f.clone(),
		receiver.clone(),
	));
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		j.c.f.store.clone(),
		receiver,
	));
	let harness = aidash::harness::Harness {
		federation: j.c.f.clone(),
	};
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
	let finished = loop {
		harness.worker_once().await.unwrap();
		let run = j.c.f.store.run(j.run.id).await.unwrap();
		assert!(
			!matches!(run.phase.as_str(), "FAILED" | "CANCELLED") && run.control != "PAUSED",
			"phase={} control={} error={:?} pending={} context={}",
			run.phase,
			run.control,
			run.error,
			run.pending,
			run.context
		);
		for entry in run.context["history"]
			.as_array()
			.into_iter()
			.flatten()
			.filter(|entry| entry["kind"] == "tool")
		{
			assert!(
				entry["result"]["error"].is_null(),
				"{}: {}",
				entry["call"]["name"],
				entry["result"]
			);
		}
		if run.phase == "COMPLETED" {
			break run;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"journey stalled: phase={} error={:?} pending={}",
			run.phase,
			run.error,
			run.pending
		);
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	};
	assert_eq!(j.effects.load(Ordering::SeqCst), 1);
	let secret = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
	assert!(!finished.context.to_string().contains(&secret));
	let requests = j.requests.lock().unwrap().clone();
	assert!(requests.len() > 10);
	for body in &requests {
		assert!(!body.to_string().contains(&secret));
		assert!(
			body.to_string().len() + 4096 + 1024 <= 128000,
			"whole provider context exceeded its limit"
		);
		let names = body["tools"]
			.as_array()
			.unwrap()
			.iter()
			.map(|tool| tool["function"]["name"].as_str().unwrap())
			.collect::<Vec<_>>();
		for name in [
			"file_search",
			"file_read",
			"shell",
			"apply_patch",
			"code_interpreter",
			"skill_load",
			"plugin_0",
		] {
			assert!(names.contains(&name), "missing {name}");
		}
		assert!(!names.contains(&"plugin_1"));
	}
	let (_, area) = request(
		&j.c.app,
		&j.c.token,
		"GET",
		&format!("/api/runs/{}/working-area", j.run.id),
		Value::Null,
	)
	.await;
	let answer = area["manifest"]
		.as_array()
		.unwrap()
		.iter()
		.find(|f| f["path"] == "answer.txt")
		.unwrap();
	let (status, read) = request(
		&j.c.app,
		&j.c.token,
		"POST",
		&format!("/api/runs/{}/files/read", j.run.id),
		json!({"file_id":answer["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "42");
	let history = finished.context["history"].as_array().unwrap();
	assert!(
		history.iter().any(|e| e["call"]["name"] == "python_poll"
			&& e["result"]["displays"]
				.as_array()
				.is_some_and(|d| !d.is_empty())),
		"a graph must be an authorized file display reference"
	);
	let (_, received) = request(
		&j.peer.app,
		&j.peer.token,
		"GET",
		&format!("/api/runs/{}/working-area", j.receiver.id),
		Value::Null,
	)
	.await;
	let copy = &received["manifest"][0];
	assert_eq!(copy["digest"], answer["digest"]);
	assert_ne!(copy["file_id"], answer["file_id"]);
	let (status, read) = request(
		&j.peer.app,
		&j.peer.token,
		"POST",
		&format!("/api/runs/{}/files/read", j.receiver.id),
		json!({"file_id":copy["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "42");
	let (status,cleaned)=request(&j.c.app,&j.c.token,"POST",&format!("/api/working-areas/{}/cleanup",area["id"].as_str().unwrap()),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"recoverable"})).await;
	assert_eq!(status, 200, "{cleaned}");
	let cleanup_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
	loop {
		let (status, current) = request(
			&j.c.app,
			&j.c.token,
			"GET",
			&format!(
				"/api/file-cleanups/{}",
				cleaned["operation_id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{current}");
		if current["state"] == "recoverable" {
			break;
		}
		assert!(tokio::time::Instant::now() < cleanup_deadline, "{current}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	let (_, read_after) = request(
		&j.peer.app,
		&j.peer.token,
		"POST",
		&format!("/api/runs/{}/files/read", j.receiver.id),
		json!({"file_id":copy["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(
		read_after["content"], "42",
		"sender cleanup cannot erase an independent transfer"
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	transfer.await.unwrap().unwrap();
	for server in j.peer_servers {
		server.abort();
		let _ = server.await;
	}
	j.peer.close().await;
	j.server.abort();
	let _ = j.server.await;
	j.c.close().await;
}
