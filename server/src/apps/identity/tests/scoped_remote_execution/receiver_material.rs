//! Receiver material is pinned and read under the live authority of a Remote Grant.
use super::*;
use aidash_domain::federation::tool_lists::DEFAULT_TOOLS;
use base64::{Engine, engine::general_purpose::STANDARD};

fn registration_path(peer: &Federation) -> String {
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let peer = node_of(peer).replace(':', "%3A").replace('/', "%2F");
	format!("/api/tenants/acme/nodes/{node}/registrations/{peer}")
}

async fn set_lists(f: &Federation, peer: &Federation, tools: &[&str], revision: i64) {
	let administrator = common::BrowserMapping::new(f, "acme", "alice").await;
	let path = format!("{}/tool-lists", registration_path(peer));
	let (status, body) = administrator
		.send(
			"PUT",
			&path,
			json!({"revision":revision,"request":{"tools":tools,"hosts":[]},"allowance":tools,"block":[]}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
}

async fn add_files(
	f: &Federation,
	app: &common::TestApplication,
	token: &str,
	files: &[(&str, &str)],
) -> Vec<(Value, Uuid)> {
	let input: Vec<_> = files
		.iter()
		.map(|(path, text)| json!({"path":path,"data":STANDARD.encode(text)}))
		.collect();
	let (status, added) = request(
		app,
		token,
		"POST",
		"/api/references/batch",
		json!({"files":input}),
	)
	.await;
	assert_eq!(status, 200, "{added}");
	let mut references = vec![];
	for attachment in added["references"].as_array().unwrap() {
		let id: Uuid = serde_json::from_value(attachment["reference_id"].clone()).unwrap();
		let file: Uuid = sqlx::query_scalar(
			"SELECT (data->'original'->>'file_id')::uuid FROM core_records WHERE id = $1 AND kind = 'reference'",
		)
		.bind(id)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
		references.push((attachment.clone(), file));
	}
	references
}

async fn approve(p: &Pair, reference: &EntityRef, revision: i64, enabled: bool) {
	let authorization = aidash_server::authorization::Authorization {
		pool: p.b.store.pool.clone(),
	};
	authorization
		.set_catalog(
			"acme",
			&aidash_domain::nodes::NodeName::derived("acme"),
			reference,
			revision,
			enabled,
			"deployment_administrator",
		)
		.await
		.unwrap();
}

async fn register_source(p: &Pair, id: &str, source: &Value) -> EntityRef {
	let source_id = EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	};
	let (status, saved) = request(
		&p.ba,
		&p.receiver_token,
		"POST",
		"/api/registry",
		json!({"id":source_id.id,"version":source_id.version,"kind":"source","name":{"en":"Receiver material"},"description":{"en":"Receiver-only content"},"schema":{},"config":{"schema_version":1,"source":source}}),
	)
	.await;
	assert_eq!(status, 200, "{saved}");
	approve(p, &source_id, 0, true).await;
	source_id
}

async fn bind_source(p: &Pair, source: &Value) -> (EntityRef, EntityRef) {
	let source_id = register_source(p, "receiver-material", source).await;
	let mut entry = receiver_agent(p).await;
	entry.config["bindings"].as_array_mut().unwrap().push(json!({
		"kind":"source","target":{"registry_deployment":p.b.config.deployment,"id":source_id.id,"version":source_id.version}
	}));
	(source_id, register_agent(p, entry).await)
}

async fn receiver_agent(p: &Pair) -> aidash_domain::registry::Entry {
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut entry =
		p.b.registry
			.get(&tenant, "research", "1.0.0")
			.await
			.unwrap();
	entry.id = "receiver-research".into();
	entry.binding_normalization = None;
	entry.config["remove_default"] = json!([]);
	entry
}

async fn register_agent(p: &Pair, entry: aidash_domain::registry::Entry) -> EntityRef {
	let agent = EntityRef {
		id: entry.id.clone(),
		version: entry.version.clone(),
	};
	let (status, saved) = request(
		&p.ba,
		&p.receiver_token,
		"POST",
		"/api/registry",
		json!(entry),
	)
	.await;
	assert_eq!(status, 200, "{saved}");
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let executor = aidash_domain::agent_subject(&node_of(&p.b), &agent.id, &agent.version);
	let mut policy = p.source_policy.clone();
	policy["subjects"][&executor] = json!({"kind":"agent","node":node});
	common::replace_policy(&p.a.store, "acme", policy).await;
	let mut policy = p.receiver_policy.clone();
	policy["subjects"][&executor] = json!({"kind":"agent","node":node});
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"no-direct-reference-read","effect":"deny","subjects":{"any":true},
		"actions":["reference.read"],"resources":{"kinds":["reference"]}
	}));
	common::replace_policy(&p.b.store, "acme", policy).await;
	approve(p, &agent, 0, true).await;
	agent
}

async fn prepare(p: &Pair, agent: &EntityRef) -> (u16, Value) {
	request(
		&p.aa,
		&p.token,
		"POST",
		&format!("/api/tasks/{}/remote-grants", p.task),
		json!({"id":p.grant,"node_id":node_of(&p.b),"agent":agent,"ttl_seconds":3600,"semantic":{"mode":"disabled"}}),
	)
	.await
}

async fn activate(p: &mut Pair) {
	let (status, activated) = request(&p.aa, &p.token, "POST", &p.activation(), json!({})).await;
	assert_eq!(status, 200, "{activated}");
	p.admission = serde_json::from_value(activated["admission_id"].clone()).unwrap();
}

fn result(run: &aidash_domain::Run, round: usize) -> Option<Value> {
	json!(run.context)["history"]
		.as_array()?
		.iter()
		.find_map(|entry| {
			let event = &entry["event"];
			(event["call"]["id"] == format!("material-{round}"))
				.then(|| event.get("result").cloned())
				.flatten()
		})
}

async fn read_result(p: &Pair, round: usize) -> Value {
	for _ in 0..20 {
		let run = p.run().await;
		if let Some(found) = result(&run, round) {
			return found;
		}
		assert_eq!(run.control.as_str(), "ACTIVE", "{:?}", run.error);
		assert!(!run.phase().is_terminal(), "{:?}", run.error);
		p.step().await;
	}
	panic!("tool result not recorded: {:?}", p.run().await);
}

fn assert_unavailable(result: &Value, message: &str) {
	assert_eq!(result["status"], "blocked", "{result}");
	assert_eq!(
		result["error"],
		json!({"code":"NOT_FOUND","message":message,"retryable":false}),
		"{result}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn remote_librarian_reads_only_its_pinned_references_and_stops_on_withdrawal(
	#[future(awt)]
	#[with(false, false, true)]
	scoped_pair: Pair,
) {
	let mut p = scoped_pair;
	let files = add_files(
		&p.b,
		&p.ba,
		&p.receiver_token,
		&[
			("catalog.md", "receiver-only index"),
			("unbound.md", "unbound content"),
		],
	)
	.await;
	common::tenant_root(&p.b, "other").await;
	let (status, policy) = request(
		&p.ba,
		&p.b.config.api_token,
		"POST",
		"/api/authorization/other",
		json!({"expected_revision":0,"bundle":{
			"tenant":"other","subjects":{"alice":{"kind":"user"}},
			"policies":[{"id":"references","effect":"allow","subjects":{"any":true},
				"actions":["reference.upload"],"resources":{"kinds":["reference"]}}]
		}}),
	)
	.await;
	assert_eq!(status, 200, "{policy}");
	let other_token = common::subject_credential(&p.b.store, "other", "alice").await;
	let foreign = add_files(
		&p.b,
		&p.ba,
		&other_token,
		&[("foreign.md", "foreign content")],
	)
	.await;
	let (source, agent) = bind_source(
		&p,
		&json!({"adapter":"reference_attachments","references":[files[0].0]}),
	)
	.await;

	let (status, refused) = prepare(&p, &agent).await;
	assert_eq!(status, 403, "{refused}");
	assert_eq!(
		refused["error"],
		"file_read, which this Agent needs, is not in the Home's Federation Tool Request"
	);
	let mut tools = DEFAULT_TOOLS.to_vec();
	tools.extend(["file_read", "file_search"]);
	set_lists(&p.a, &p.b, &tools, 1).await;
	let (status, refused) = prepare(&p, &agent).await;
	assert_eq!(status, 403, "{refused}");
	assert_eq!(
		refused["error"],
		"file_read, which this Agent needs, is not in the receiving Node's Federation Tool Allowance"
	);
	let denied: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM authorization_decisions WHERE decision->>'reason' = 'required_tool_not_listed'",
	)
	.fetch_one(p.b.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(denied > 0);
	set_lists(&p.b, &p.a, &tools, 1).await;
	let (status, grant) = prepare(&p, &agent).await;
	assert_eq!(status, 200, "{grant}");
	activate(&mut p).await;

	p.model.material_calls.lock().await.extend([
		(
			"file_search".into(),
			json!({"query":"","mode":"path","scope":"references"}),
		),
		(
			"file_read".into(),
			json!({"file_id":files[0].1,"representation":"text"}),
		),
		(
			"file_read".into(),
			json!({"file_id":files[1].1,"representation":"text"}),
		),
		(
			"file_read".into(),
			json!({"file_id":foreign[0].1,"representation":"text"}),
		),
	]);
	let found = read_result(&p, 1).await;
	let matches = found["matches"].as_array().unwrap();
	assert_eq!(matches.len(), 1, "{found}");
	assert_eq!(matches[0]["path"], "references/catalog.md");
	assert_eq!(matches[0]["file_id"], json!(files[0].1));
	let read = read_result(&p, 2).await;
	assert_eq!(read["content"], "receiver-only index");
	for round in [3, 4] {
		assert_unavailable(&read_result(&p, round).await, "file unavailable");
	}

	let inferences = p.requests.lock().await.len();
	approve(&p, &source, 1, false).await;
	let stopped = step_until_paused(&p).await;
	assert_eq!(stopped.control.as_str(), "PAUSED");
	assert_eq!(p.requests.lock().await.len(), inferences);
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_remote_legacy_run_lists_and_loads_only_its_bound_skill(
	#[future(awt)]
	#[with(false, false, true)]
	scoped_pair: Pair,
) {
	let mut p = scoped_pair;
	let skill = aidash_domain::capabilities::skills::imported(
		"fixture".into(),
		"---\nname: remote-skill\ndescription: Bound instructions\n---\nUse the receiver's pinned procedure.\n".into(),
		vec![],
	)
	.unwrap();
	let (_, agent) = bind_source(
		&p,
		&json!({"adapter":"skill_attachments","attachments":[skill]}),
	)
	.await;
	let unbound = aidash_domain::capabilities::skills::imported(
		"fixture".into(),
		"---\nname: unbound-skill\ndescription: Unbound instructions\n---\nThis Skill belongs to another Source.\n".into(),
		vec![],
	)
	.unwrap();
	register_source(
		&p,
		"unbound-skill",
		&json!({"adapter":"skill_attachments","attachments":[unbound]}),
	)
	.await;
	let mut tools = DEFAULT_TOOLS.to_vec();
	tools.extend(["skill_list", "skill_load"]);
	set_lists(&p.a, &p.b, &tools, 1).await;
	set_lists(&p.b, &p.a, &tools, 1).await;
	let (status, grant) = prepare(&p, &agent).await;
	assert_eq!(status, 200, "{grant}");
	activate(&mut p).await;
	p.model.material_calls.lock().await.extend([
		("skill_list".into(), json!({})),
		(
			"skill_load".into(),
			json!({"skill_id":skill.skill_id,"expected_digest":skill.digest}),
		),
		(
			"skill_load".into(),
			json!({"skill_id":unbound.skill_id,"expected_digest":unbound.digest}),
		),
	]);
	let listed = read_result(&p, 1).await;
	let skills = listed["skills"].as_array().unwrap();
	assert_eq!(skills.len(), 1, "{listed}");
	assert_eq!(skills[0]["skill_id"], json!(skill.skill_id));
	let loaded = read_result(&p, 2).await;
	assert_eq!(loaded["content"], skill.instructions);
	assert_unavailable(&read_result(&p, 3).await, "skill unavailable");
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_explicit_never_remote_tool_is_refused_with_its_public_name(
	#[future(awt)]
	#[with(false, false, true)]
	scoped_pair: Pair,
) {
	let p = scoped_pair;
	let mut entry = receiver_agent(&p).await;
	let tool = aidash_domain::registry::bindings::QualifiedRef::builtin(
		p.b.config.deployment.as_str(),
		"duty_list",
	);
	entry.config["bindings"]
		.as_array_mut()
		.unwrap()
		.push(json!({
			"kind":"tool","target":tool
		}));
	let agent = register_agent(&p, entry).await;
	let (status, body) = prepare(&p, &agent).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(body["code"], "tool_never_remote");
	assert_eq!(body["tool"], "duty_list");
	assert_eq!(
		body["error"],
		"the receiving Node cannot run duty_list, which this Agent uses, remotely"
	);
	let receiver = common::default_node(&p.b.store, "acme").await;
	let (status, wire) = common::peer_request(
		&p.ba,
		(&p.a, "acme"),
		&receiver,
		"alice",
		"/federation/v0.1/scoped/execution/inspect",
		json!({"task_id":p.task,"agent":agent,"requirements":{}}),
	)
	.await;
	assert_eq!(status, 403, "{wire}");
	assert_eq!(wire, json!({"code":"tool_never_remote","tool":"duty_list"}));
	p.close().await;
}

#[rstest::rstest]
#[case::receiver_run_only(false, "run_only")]
#[case::receiver_add(false, "add")]
#[case::receiver_decline(false, "decline")]
#[case::receiver_block(false, "block")]
#[case::home_run_only(true, "run_only")]
#[case::home_add(true, "add")]
#[case::home_decline(true, "decline")]
#[tokio::test]
async fn a_list_holder_can_answer_a_real_remote_tool_ask(
	#[future(awt)] scoped_pair: Pair,
	#[case] home: bool,
	#[case] choice: &str,
) {
	let p = scoped_pair;
	let (local, peer) = if home { (&p.a, &p.b) } else { (&p.b, &p.a) };
	let tools: Vec<_> = DEFAULT_TOOLS
		.iter()
		.copied()
		.filter(|tool| *tool != "thread_message")
		.collect();
	set_lists(local, peer, &tools, 1).await;
	let waiting = step_until_paused(&p).await;
	let paused = waiting.recovery.paused_call.as_ref().unwrap();
	assert_eq!(paused.ask.as_ref().unwrap().key, "thread_message");
	assert_eq!(paused.ask.as_ref().unwrap().lacking.home, home);
	assert_eq!(paused.ask.as_ref().unwrap().lacking.receiver, !home);
	let administrator = common::BrowserMapping::new(local, "acme", "alice").await;
	let asks = format!("{}/tool-asks", registration_path(peer));
	let (status, listed) = administrator.send("GET", &asks, Value::Null).await;
	assert_eq!(status, 200, "{listed}");
	let asks_on_page = listed["asks"].as_array().unwrap();
	assert_eq!(asks_on_page.len(), 1, "{listed}");
	let ask = &asks_on_page[0];
	let id = if home { p.grant } else { p.admission };
	assert_eq!(ask["id"], json!(id));
	assert_eq!(ask["side"], if home { "home" } else { "receiver" });
	assert_eq!(ask["call"], paused.key);
	let answer = format!("{asks}/{id}/answer");
	let response = json!({"choice":choice,"key":"thread_message","call":paused.key});

	let (status, stale) = administrator
		.send(
			"POST",
			&answer,
			json!({"choice":choice,"key":"thread_message","call":"stale-page"}),
		)
		.await;
	assert_eq!(status, 409, "{stale}");
	// Each side's resume must respect a pause imposed by a person or a Safety Stop.
	for source in ["tenant", "deployment_administrator"] {
		sqlx::query("UPDATE task_executions SET control_source = $1 WHERE id = $2")
			.bind(source)
			.bind(p.admission)
			.execute(p.b.store.pool.fixture_driver())
			.await
			.unwrap();
		let (status, refused) = administrator.send("POST", &answer, response.clone()).await;
		assert_eq!(status, 409, "{source}: {refused}");
		assert_eq!(p.run().await.control.as_str(), "PAUSED");
	}
	sqlx::query("UPDATE task_executions SET control_source = 'system' WHERE id = $1")
		.bind(p.admission)
		.execute(p.b.store.pool.fixture_driver())
		.await
		.unwrap();

	let policy = if home {
		&p.source_policy
	} else {
		&p.receiver_policy
	};
	let mut denied = policy.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({
		"id":"withdraw-list-management","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["node.federation_tools.manage"],"resources":{"kinds":["node_registration"]}
	}));
	common::replace_policy(&local.store, "acme", denied).await;
	let (status, refused) = administrator.send("POST", &answer, response.clone()).await;
	assert_eq!(status, 403, "{refused}");
	assert_eq!(p.run().await.control.as_str(), "PAUSED");
	common::replace_policy(&local.store, "acme", policy.clone()).await;

	let (status, answered) = administrator.send("POST", &answer, response).await;
	assert_eq!(status, 200, "{answered}");
	assert_eq!(answered["choice"], choice);
	assert_eq!(answered["resumed"], true);
	assert_eq!(p.run().await.control.as_str(), "ACTIVE");

	let (request, allowance, block): (Value, Value, Value) = sqlx::query_as(
		"SELECT tool_request, tool_allowance, tool_block FROM node_registrations WHERE source_node = $1",
	)
	.bind(node_of(peer))
	.fetch_one(local.store.pool.fixture_driver())
	.await
	.unwrap();
	let list = if home { &request["tools"] } else { &allowance };
	let key = json!("thread_message");
	assert_eq!(list.as_array().unwrap().contains(&key), choice == "add");
	assert_eq!(block.as_array().unwrap().contains(&key), choice == "block");
	let additions_sql = if home {
		"SELECT COALESCE(tool_request_additions, '[]'::jsonb) FROM authorization_remote_grants WHERE id = $1"
	} else {
		"SELECT COALESCE(run_tool_allowance, '[]'::jsonb) FROM authorization_remote_admissions WHERE id = $1"
	};
	let additions: Value = sqlx::query_scalar(additions_sql)
		.bind(id)
		.fetch_one(local.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		additions.as_array().unwrap().contains(&key),
		choice == "run_only"
	);
	let (status, listed) = administrator.send("GET", &asks, Value::Null).await;
	assert_eq!(status, 200, "{listed}");
	assert_eq!(listed, json!({"asks":[]}));
	for _ in 0..16 {
		if p.run().await.phase().is_terminal() {
			break;
		}
		p.step().await;
	}
	let run = p.run().await;
	assert_eq!(run.phase().as_str(), "COMPLETED", "{:?}", run.error);
	let task = p.a.store.task(&TEST_FIXTURE, p.task).await.unwrap();
	let snapshot =
		p.a.store
			.snapshot(&TEST_FIXTURE, task.thread_id)
			.await
			.unwrap();
	let delivered = snapshot
		.messages
		.iter()
		.any(|message| message.content == "Scoped remote progress");
	assert_eq!(delivered, matches!(choice, "run_only" | "add"));
	p.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_home_checks_its_request_before_staging_a_message_without_a_delegating_area(
	#[future(awt)] scoped_pair: Pair,
) {
	let p = scoped_pair;
	let bytes = b"message attachment";
	let key = "receiver-message-with-file";
	let open = json!({
		"grant_id":p.grant,"admission_id":p.admission,"key":key,
		"purpose":{"kind":"message","content":"Report from the remote Run"},
		"files":[{"path":"report.txt","digest":aidash_server::capabilities::objects::digest(bytes),"size":bytes.len(),"media_type":"text/plain"}]
	});
	let destination = common::default_node(&p.a.store, "acme").await;
	let path = "/federation/v0.1/scoped/execution/deliveries/prepare";
	let (status, denied) =
		common::peer_request(&p.aa, (&p.b, "acme"), &destination, "", path, open.clone()).await;
	assert_eq!(status, 400, "{denied}");
	assert_eq!(denied["error"], "this tool is not available on this Node");

	let mut tools = DEFAULT_TOOLS.to_vec();
	tools.push("thread_message:files");
	set_lists(&p.a, &p.b, &tools, 1).await;
	let (status, staged) =
		common::peer_request(&p.aa, (&p.b, "acme"), &destination, "", path, open.clone()).await;
	assert_eq!(status, 200, "{staged}");
	assert_eq!(staged["state"], "staging", "{staged}");
	let (status, repeated) =
		common::peer_request(&p.aa, (&p.b, "acme"), &destination, "", path, open).await;
	assert_eq!(status, 200, "{repeated}");
	assert_eq!(repeated, staged);
	p.close().await;
}
