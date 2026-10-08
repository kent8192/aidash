//! A provider-driven journey: tool selection and tool arguments travel through
//! the real Harness, authorization guards, journal, and isolated runtime.
use super::*;
use aidash_server::{
	context::ContextEvent,
	domain::{RunControl, RunPhase, RunState, WaitingState},
};
use reinhardt::ServerRouter as Router;
use std::sync::{
	Mutex,
	atomic::{AtomicUsize, Ordering},
};
use upstream_fixtures::handler;

struct Journey {
	c: CoreFixture,
	peer: CoreFixture,
	receiver: aidash_server::domain::Run,
	peer_servers: Vec<tokio::task::JoinHandle<()>>,
	run: aidash_server::domain::Run,
	server: Arc<reinhardt::test::fixtures::server::TestServerGuard>,
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
async fn journey_fixture(
	#[from(journey_state)] _state: JourneyState,
	#[from(journey_router)]
	#[with(_state.recipient.clone(),_state.requests.clone(),_state.effects.clone())]
	_router: Arc<Router>,
	#[from(upstream_fixtures::provider_transport)]
	#[with(_router.clone())]
	_provider: upstream_fixtures::UpstreamFuture,
	#[from(provider_endpoint)]
	#[with(_provider.clone())]
	_endpoint: EndpointFuture,
	#[from(capability_fixture)]
	#[with("aidash://journey",_endpoint.clone(),true)]
	_initial_core: CoreFuture,
	#[from(journey_core)]
	#[with(_initial_core.clone())]
	_core: CoreFuture,
	#[future(awt)]
	#[from(super::transfer_tests::connected_nodes)]
	#[with(3,"aidash://journey","aidash://journey-_state.recipient",_core.clone())]
	peers: super::transfer_tests::ConnectedNodes,
) -> Journey {
	let JourneyState {
		recipient,
		requests,
		effects,
	} = _state;
	let super::transfer_tests::ConnectedNodes {
		a: c,
		b: peer,
		servers: peer_servers,
		..
	} = peers;
	let server = _provider.await;
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
	#[future]
	#[from(running_journey_fixture)]
	journey_fixture: (Journey, CapabilityWorker, CapabilityWorker),
) {
	let (j, worker, transfer) = Box::pin(journey_fixture).await;
	let stop = worker.stop.clone();
	let harness = aidash_server::harness::Harness {
		federation: j.c.f.clone(),
	};
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
	let mut answered_approval = false;
	let finished = loop {
		harness.worker_once().await.unwrap();
		let run = j.c.f.store.run(j.run.id).await.unwrap();
		assert!(
			!matches!(run.phase(), RunPhase::Failed | RunPhase::Cancelled)
				&& run.control != RunControl::Paused,
			"phase={} control={} error={:?} state={:?} context={}",
			run.phase(),
			run.control,
			run.error,
			run.state,
			run.context
		);
		for entry in &run.context.history {
			if let ContextEvent::Tool { call, result } = entry {
				assert!(result["error"].is_null(), "{}: {}", call.name, result);
			}
		}
		if let RunState::Waiting(waiting) = &run.state
			&& let WaitingState::ExternalApproval {
				request_id, call, ..
			} = waiting.as_ref()
		{
			assert!(
				!answered_approval,
				"the exact external action must need only one approval"
			);
			assert_eq!(call.name, "plugin_0");
			assert_eq!(call.arguments, json!({"result":42}));
			assert_eq!(j.effects.load(Ordering::SeqCst), 0);
			let human = aidash_application::ports::execution::ExecutionStore::human_request_by_id(
				&j.c.f.store,
				*request_id,
			)
			.await
			.unwrap();
			assert_eq!(human.kind, "APPROVAL_REQUIRED");
			assert!(
				human
					.prompt
					.starts_with("Approve this exact external tool action once? Tool: http@1.1.0;")
			);
			let (status, result) = request(
				&j.c.app,
				&j.c.token,
				"POST",
				&format!("/api/human-requests/{request_id}/answer"),
				json!({"approved":true}),
			)
			.await;
			assert_eq!(status, 200, "{result}");
			answered_approval = true;
		}
		if run.phase() == RunPhase::Completed {
			break run;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"journey stalled: phase={} error={:?} state={:?}",
			run.phase(),
			run.error,
			run.state
		);
		tokio::time::sleep(std::time::Duration::from_millis(20)).await;
	};
	assert!(answered_approval);
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
	assert!(
		finished.context.history.iter().any(|event| matches!(
			event,
			ContextEvent::Tool { call, result }
				if call.name == "python_poll"
					&& result["displays"].as_array().is_some_and(|displays| !displays.is_empty())
		)),
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
	drop(j.server);
	j.c.close().await;
}

#[rstest::fixture]
fn journey_recipient() -> Arc<Mutex<Value>> {
	Arc::new(Mutex::new(Value::Null))
}
#[rstest::fixture]
fn journey_requests() -> Arc<Mutex<Vec<Value>>> {
	Arc::new(Mutex::new(Vec::new()))
}
#[rstest::fixture]
fn journey_effects() -> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}
#[rstest::fixture]
fn journey_router(
	#[from(journey_recipient)] recipient: Arc<Mutex<Value>>,
	#[from(journey_requests)] requests: Arc<Mutex<Vec<Value>>>,
	#[from(journey_effects)] effects: Arc<AtomicUsize>,
) -> Arc<Router> {
	let target = recipient.clone();
	let capture = requests.clone();
	let counter = effects.clone();
	let server = Router::new().handler("/v1/chat/completions", handler(http::Method::POST, move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
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
			reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"},"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
		}
	})).handler("/effect", handler(http::Method::POST, move |request: reinhardt::Request| {let headers = request.headers.clone();let _body = request.json::<Value>().unwrap();
		let counter = counter.clone();
		async move {
			assert_eq!(headers.get("authorization").unwrap(), &format!("Bearer {}", std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()));
			counter.fetch_add(1, Ordering::SeqCst);
			reinhardt::Response::ok().with_json(&json!({"saved":true})).unwrap()
		}
	}));
	Arc::new(server)
}
#[rstest::fixture]
fn journey_core(#[from(capability_fixture)] core: CoreFuture) -> CoreFuture {
	async move {
		let mut c = core.await;
		let mut tool = c.f.registry.get("http", "1.0.0").await.unwrap();
		tool.version = "1.1.0".into();
		tool.config["transport"]["credential_env"] = json!("AIDASH_SECRET_TEST_PEER");
		let mut agent = c.f.registry.get("research", "1.1.0").await.unwrap();
		agent.version = "1.2.0".into();
		agent.binding_normalization = None;
		agent.config["bindings"].as_array_mut().unwrap().push(json!({"kind":"tool","target":{"registry_node":c.f.config.node_id,"id":"http","version":"1.1.0"},"alias":"plugin_0","narrow":{}}));
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
			[aidash_server::domain::qualified_agent(&c.f.config.node_id, "research", "1.2.0")] =
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
		c
	}
	.boxed()
	.shared()
}

#[rstest::fixture]
async fn running_journey_fixture(
	#[future] journey_fixture: Journey,
	worker_control: WorkerControl,
) -> (Journey, CapabilityWorker, CapabilityWorker) {
	let j = Box::pin(journey_fixture).await;
	let transfer = CapabilityWorker {
		stop: worker_control.stop.clone(),
		handle: tokio::spawn(aidash_server::capabilities::transfer::run(
			j.c.f.clone(),
			worker_control.receiver.clone(),
		)),
	};
	let worker = CapabilityWorker {
		stop: worker_control.stop,
		handle: tokio::spawn(aidash_server::capabilities::operations::run(
			j.c.f.store.clone(),
			worker_control.receiver,
		)),
	};
	(j, worker, transfer)
}

#[derive(Clone)]
struct JourneyState {
	recipient: Arc<Mutex<Value>>,
	requests: Arc<Mutex<Vec<Value>>>,
	effects: Arc<AtomicUsize>,
}
#[rstest::fixture]
fn journey_state(
	#[from(journey_recipient)] recipient: Arc<Mutex<Value>>,
	#[from(journey_requests)] requests: Arc<Mutex<Vec<Value>>>,
	#[from(journey_effects)] effects: Arc<AtomicUsize>,
) -> JourneyState {
	JourneyState {
		recipient,
		requests,
		effects,
	}
}
