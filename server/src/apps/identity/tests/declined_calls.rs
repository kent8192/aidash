//! A person declines the tool call a paused Run stopped on, and the Run goes on without it (#763).
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_domain::{
	provider::{ModelResponse, ToolCall},
	run_state::{PausedCall, RecoveryState, RunState, ToolCallState, encode},
	tool::RefusalKind,
};
use aidash_server::federation::Federation;
use serde_json::{Value, json};
use uuid::Uuid;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;

const LISTED: &str = "listed-agent";

/// A Tenant whose `alice` started one Run, which has read an approved definition
/// outside its own Binding, as `agent_discover` records one.
struct World {
	f: Federation,
	app: common::TestApplication,
	run: Uuid,
	task: aidash_domain::Task,
	child_agent: aidash_server::registry::Entry,
	alice: String,
	_runtime: common::RuntimeFixture,
}

async fn world(fixture: common::ApplicationFixture) -> World {
	let (f, _, _) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let alice = common::tenant_token(&f.store, "acme").await;
	let acme = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let workspace = f
		.store
		.create_workspace(&acme, "alice", "Run Control", "Stop a Run")
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			&acme,
			workspace.id,
			&aidash_server::domain::NewTask {
				title: "Work".into(),
				description: "A Run whose owner stops it".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let mut registered = vec![];
	for (kind, id, config) in [
		(
			"model",
			"fixture-model",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:1/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"agent",
			"fixture-agent",
			json!({"schema_version":1,"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Fixture","bindings":[],"remove_default":[]}),
		),
		(
			"agent",
			LISTED,
			json!({"schema_version":1,"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Listed","bindings":[],"remove_default":[]}),
		),
	] {
		registered.push(
			f.registry
				.register(
					acme.tenant_name().unwrap(),
					serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"schema":{},"config":config})).unwrap(),
				)
				.await
				.unwrap(),
		);
	}
	common::approve_agent(&f, "acme", &registered[2]).await;
	let run = common::admitted_run(&f, "acme", &task, &registered[1]).await;
	sqlx::query(
		"INSERT INTO authorization_run_registry_reads (run_id, entry_id, entry_version, tenant) \
		 VALUES ($1, $2, $3, 'acme')",
	)
	.bind(run.id)
	.bind(LISTED)
	.bind("1.0.0")
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	World {
		f,
		app,
		run: run.id,
		task,
		child_agent: registered[2].clone(),
		alice,
		_runtime: fixture.runtime,
	}
}

impl World {
	async fn get(&self, token: &str) -> (u16, Value) {
		common::request(
			&self.app,
			token,
			"GET",
			&format!("/api/runs/{}", self.run),
			Value::Null,
		)
		.await
	}

	async fn control(&self, token: &str, action: &str) -> (u16, Value) {
		common::request(
			&self.app,
			token,
			"POST",
			&format!("/api/runs/{}/control", self.run),
			json!({"action":action}),
		)
		.await
	}

	async fn management(&self, token: &str) -> (u16, Value) {
		common::request(
			&self.app,
			token,
			"GET",
			&format!("/api/runs/{}/management", self.run),
			Value::Null,
		)
		.await
	}

	/// `subjects` may not read the listed definition, which the Run has read. Since #769 a
	/// withdrawn Catalog Approval no longer hides what a Run read; a denied `registry.read` does.
	async fn keep_definition_from(&self, subjects: &[&str]) {
		let current = aidash_server::authorization::Authorization {
			pool: self.f.store.pool.clone(),
		}
		.snapshot("acme")
		.await
		.unwrap();
		let mut bundle = serde_json::to_value(&current.bundle).unwrap();
		bundle["policies"].as_array_mut().unwrap().push(json!({"id":"no-definition","effect":"deny","subjects":{"ids":subjects},"actions":["registry.read"],"resources":{"kinds":["agent"],"ids":[LISTED]}}));
		common::replace_policy(&self.f.store, "acme", bundle).await;
	}
}

const EPOCH: i64 = 5;

impl World {
	fn key(&self) -> String {
		format!("{}:{EPOCH}:0", self.run)
	}

	/// The Run is paused on its only call, as an Authority Pause leaves it. `recorded` is whether
	/// the pause recorded the call; a plain pause by a person does not.
	async fn pause(&self, recorded: bool) {
		let state = RunState::ToolCall(Box::new(ToolCallState {
			response: ModelResponse {
				tool_calls: vec![ToolCall {
					id: "call-0".into(),
					name: "task_delegate".into(),
					arguments: json!({"task_id": Uuid::nil()}),
				}],
				..Default::default()
			},
			response_epoch: EPOCH,
			..Default::default()
		}));
		let recovery = RecoveryState {
			paused_call: recorded.then(|| PausedCall {
				key: self.key(),
				index: 0,
				replay_safe: true,
				tool: "task_delegate".into(),
				kind: RefusalKind::Policy,
			}),
			..Default::default()
		};
		let phase = serde_json::to_value(state.phase()).unwrap();
		sqlx::query(
			"UPDATE runs SET pending = $1, phase = $2, control = 'PAUSED', error = $3 WHERE id = $4",
		)
		.bind(encode(&state, &recovery).unwrap())
		.bind(phase.as_str().unwrap())
		.bind("task_delegate: task.delegate was denied")
		.bind(self.run)
		.execute(self.f.store.pool.fixture_driver())
		.await
		.unwrap();
	}

	async fn journal(&self, status: &str) {
		sqlx::query(
			"INSERT INTO invocations (idempotency_key, run_id, tool, input, status, replay_safe, tenant) \
			 VALUES ($1, $2, 'task_delegate', $3, $4, true, 'acme')",
		)
		.bind(self.key())
		.bind(self.run)
		.bind(json!({"task_id": Uuid::nil()}))
		.bind(status)
		.execute(self.f.store.pool.fixture_driver())
		.await
		.unwrap();
	}

	async fn decline(&self, token: &str, call: &str) -> (u16, Value) {
		common::request(
			&self.app,
			token,
			"POST",
			&format!("/api/runs/{}/declined-calls", self.run),
			json!({"call": call}),
		)
		.await
	}

	async fn row(&self) -> Option<(String, Option<Value>)> {
		sqlx::query_as::<_, (String, Option<Value>)>(
			"SELECT status, result FROM invocations WHERE idempotency_key = $1",
		)
		.bind(self.key())
		.fetch_optional(self.f.store.pool.fixture_driver())
		.await
		.unwrap()
	}

	async fn events(&self) -> Vec<(String, Value)> {
		sqlx::query_as::<_, (String, Value)>(
			"SELECT kind, data FROM events WHERE data->>'run_id' = $1",
		)
		.bind(self.run.to_string())
		.fetch_all(self.f.store.pool.fixture_driver())
		.await
		.unwrap()
	}

	async fn credential(&self) -> Uuid {
		sqlx::query_scalar("SELECT credential_id FROM authorization_execution WHERE run_id = $1")
			.bind(self.run)
			.fetch_one(self.f.store.pool.fixture_driver())
			.await
			.unwrap()
	}

	/// Another real credential of `alice`, which the Run's grant can name instead of her own.
	async fn other_credential(&self) -> Uuid {
		let issued = common::issued_credential(&self.f.store, "acme", "alice").await;
		issued["credential"]["id"]
			.as_str()
			.unwrap()
			.parse()
			.unwrap()
	}

	async fn set_credential(&self, id: Uuid) {
		sqlx::query("UPDATE authorization_execution SET credential_id = $1 WHERE run_id = $2")
			.bind(id)
			.bind(self.run)
			.execute(self.f.store.pool.fixture_driver())
			.await
			.unwrap();
	}
}

fn sentence() -> String {
	RefusalKind::Policy.declined_sentence()
}

#[rstest::rstest]
#[tokio::test]
async fn a_person_can_decline_the_same_refusal_for_the_rest_of_the_run(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	w.pause(true).await;
	let (status, body) = common::request(
		&w.app,
		&w.alice,
		"POST",
		&format!("/api/runs/{}/declined-calls", w.run),
		json!({"call": w.key(), "rest_of_run": true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let events = w.events().await;
	let event = events
		.iter()
		.find(|(kind, _)| kind == "run.call_declined")
		.unwrap();
	assert_eq!(event.1["rest_of_run"], true);
	assert_eq!(w.row().await.unwrap().0, "COMPLETED");
}

#[rstest::rstest]
#[tokio::test]
async fn a_new_child_run_does_not_inherit_the_parent_decision(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	w.pause(true).await;
	let (status, body) = common::request(
		&w.app,
		&w.alice,
		"POST",
		&format!("/api/runs/{}/declined-calls", w.run),
		json!({"call":w.key(),"rest_of_run":true}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let scope = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let child =
		w.f.store
			.create_task(
				&scope,
				w.task.workspace_id,
				&aidash_domain::NewTask {
					title: "Child".into(),
					description: "Child task".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: Some(w.task.id),
				},
				"alice",
				None,
			)
			.await
			.unwrap();
	let child_run = common::admitted_run(&w.f, "acme", &child, &w.child_agent).await;
	assert!(child_run.recovery.standing_declines.is_empty());
}

#[rstest::rstest]
#[case::no_row(false)]
#[case::refused_row(true)]
#[tokio::test]
async fn the_runs_person_declines_the_call_and_the_run_goes_on(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	#[case] refused: bool,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.pause(true).await;
	if refused {
		w.journal("REFUSED").await;
	}
	// The page offers the call to someone who may decline it, and never inside `recovery`.
	let (status, page) = w.get(&alice).await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(
		page["declinable_call"],
		json!({"call": w.key(), "tool": "task_delegate", "kind": "policy"})
	);
	assert!(
		!page["run"]["recovery"].to_string().contains(&w.key()),
		"{page}"
	);
	// The credential is renewed to the person's own by their own decline.
	let before = w.other_credential().await;
	w.set_credential(before).await;

	let (status, declined) = w.decline(&alice, &w.key()).await;
	assert_eq!(status, 200, "{declined}");
	assert_eq!(declined["control"], "ACTIVE");
	assert_ne!(w.credential().await, before);

	let (row, result) = w.row().await.expect("a declined call has a row");
	assert_eq!(row, "COMPLETED");
	assert_eq!(result, Some(json!({"error": sentence()})));
	let events = w.events().await;
	let kinds: Vec<&str> = events.iter().map(|(kind, _)| kind.as_str()).collect();
	assert!(kinds.contains(&"run.call_declined"), "{kinds:?}");
	assert!(kinds.contains(&"tool.completed"), "{kinds:?}");
	assert!(!kinds.contains(&"tool.started"), "{kinds:?}");
	let (_, event) = events
		.iter()
		.find(|(kind, _)| kind == "run.call_declined")
		.unwrap();
	assert_eq!(event["by"], "alice");
	assert_eq!(event["tool"], "task_delegate");
	assert_eq!(event["kind"], "policy");
	assert_eq!(event["call"], w.key());
	assert!(!event.to_string().contains("task.delegate"), "{event}");
	// Nothing is offered once the Run is no longer paused on it.
	let (_, page) = w.get(&alice).await;
	assert!(page.get("declinable_call").is_none(), "{page}");
}

#[rstest::rstest]
#[case::started("STARTED")]
#[case::completed("COMPLETED")]
#[tokio::test]
async fn a_call_that_may_have_run_is_not_offered_and_answers_conflict(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	#[case] status: &str,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	// The pause records a call only when no row says it started; here a row says so, which is
	// the state a `STARTED` or finished call leaves.
	w.pause(false).await;
	w.journal(status).await;
	let (code, page) = w.get(&alice).await;
	assert_eq!(code, 200, "{page}");
	assert!(page.get("declinable_call").is_none(), "{page}");
	let (code, body) = w.decline(&alice, &w.key()).await;
	assert_eq!(code, 409, "{body}");
	assert_eq!(w.management(&alice).await.1["control"], "PAUSED");
}

#[rstest::rstest]
#[tokio::test]
async fn a_pause_that_is_not_an_authority_pause_on_a_call_answers_conflict(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	// A person's plain pause, a stage pause, `RunAuthority` and `TenantDeletion` record no call.
	w.pause(false).await;
	let (_, page) = w.get(&alice).await;
	assert!(page.get("declinable_call").is_none(), "{page}");
	let (status, body) = w.decline(&alice, &w.key()).await;
	assert_eq!(status, 409, "{body}");
	assert!(w.row().await.is_none());
}

#[rstest::rstest]
#[tokio::test]
async fn a_stale_call_key_answers_conflict_and_changes_nothing(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.pause(true).await;
	let (status, body) = w.decline(&alice, &format!("{}:{EPOCH}:9", w.run)).await;
	assert_eq!(status, 409, "{body}");
	assert!(w.row().await.is_none());
	assert_eq!(w.management(&alice).await.1["control"], "PAUSED");
	let (status, body) = w.decline(&alice, "").await;
	assert!(status == 400 || status == 422, "{status} {body}");
	let (status, body) = w.decline(&alice, &"x".repeat(300)).await;
	assert!(status == 400 || status == 422, "{status} {body}");
	// An unknown field is refused rather than ignored.
	let unknown = w
		.app
		.api_http
		.request(
			"POST".parse().unwrap(),
			&format!("/api/runs/{}/declined-calls", w.run),
		)
		.header("Authorization", format!("Bearer {alice}"))
		.json(&json!({"call": w.key(), "action": "resume"}))
		.send()
		.await
		.unwrap();
	assert!(
		matches!(unknown.status_code(), 400 | 422),
		"{}",
		unknown.status_code()
	);
	assert!(w.row().await.is_none());
}

#[rstest::rstest]
#[tokio::test]
async fn a_run_blocked_by_what_it_read_answers_like_resuming(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.pause(true).await;
	w.keep_definition_from(&["alice"]).await;
	let (status, resume) = w.control(&alice, "resume").await;
	assert_eq!(status, 409, "{resume}");
	let (status, body) = w.decline(&alice, &w.key()).await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["run_reads"], resume["run_reads"]);
	assert!(w.row().await.is_none());
	assert_eq!(w.management(&alice).await.1["control"], "PAUSED");
}

#[rstest::rstest]
#[tokio::test]
async fn a_sentence_sent_first_is_an_ordinary_run_message_to_a_paused_run(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.pause(true).await;
	let (status, sent) = common::request(
		&w.app,
		&alice,
		"POST",
		&format!("/api/runs/{}/message", w.run),
		json!({"content": "Use the shared notes instead.", "idempotency_key": Uuid::new_v4().to_string()}),
	)
	.await;
	assert_eq!(status, 200, "{sent}");
	let (status, declined) = w.decline(&alice, &w.key()).await;
	assert_eq!(status, 200, "{declined}");
	let messages: i64 = sqlx::query_scalar("SELECT count(*) FROM run_inputs WHERE run_id = $1")
		.bind(w.run)
		.fetch_one(w.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(messages, 1);
}

impl World {
	async fn edit_policy(&self, edit: impl FnOnce(&mut Value)) {
		let current = aidash_server::authorization::Authorization {
			pool: self.f.store.pool.clone(),
		}
		.snapshot("acme")
		.await
		.unwrap();
		let mut bundle = serde_json::to_value(&current.bundle).unwrap();
		edit(&mut bundle);
		common::replace_policy(&self.f.store, "acme", bundle).await;
	}

	/// Another administrator of the Tenant, who is not the Run's person.
	async fn add_administrator(&self, name: &str) -> String {
		self.edit_policy(|bundle| {
			bundle["groups"]["administrators"] = json!({"roles": [], "assignable": true});
			bundle["subjects"][name] = json!({"kind":"user","groups":["administrators"]});
		})
		.await;
		common::subject_credential(&self.f.store, "acme", name).await
	}

	/// A member who may control Runs, as every member may, and is denied `run.decline_call`.
	async fn add_member_denied_decline(&self, name: &str) -> String {
		self.edit_policy(|bundle| {
			bundle["groups"]["members"] = json!({"roles": [], "assignable": true});
			bundle["subjects"][name] = json!({"kind":"user","groups":["members"]});
			bundle["policies"].as_array_mut().unwrap().push(json!({
				"id":"no-decline","effect":"deny","subjects":{"ids":[name]},
				"actions":["run.decline_call"],"resources":{"kinds":["*"]}
			}));
		})
		.await;
		common::subject_credential(&self.f.store, "acme", name).await
	}

	async fn lift_the_denial(&self) {
		self.edit_policy(|bundle| {
			bundle["policies"]
				.as_array_mut()
				.unwrap()
				.retain(|policy| policy["id"] != "no-decline");
		})
		.await;
	}

	/// The Run was admitted without a Working Area. A Run with one is readable only by the
	/// area's owner, which is the Run's person, so nobody else can see it.
	async fn drop_working_area(&self) {
		sqlx::query("DELETE FROM core_runs WHERE run_id = $1")
			.bind(self.run)
			.execute(self.f.store.pool.fixture_driver())
			.await
			.unwrap();
	}
}

#[rstest::rstest]
#[tokio::test]
async fn another_administrator_declines_and_the_runs_credential_is_left_alone(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let erin = w.add_administrator("erin").await;
	w.drop_working_area().await;
	w.pause(true).await;
	let before = w.other_credential().await;
	w.set_credential(before).await;

	let (status, page) = w.get(&erin).await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(page["declinable_call"]["tool"], "task_delegate");
	let (status, declined) = w.decline(&erin, &w.key()).await;
	assert_eq!(status, 200, "{declined}");
	assert_eq!(declined["control"], "ACTIVE");
	// The Run goes on in its person's name, with the credential it had.
	assert_eq!(w.credential().await, before);
	let (row, result) = w.row().await.expect("a declined call has a row");
	assert_eq!(row, "COMPLETED");
	assert_eq!(result, Some(json!({"error": sentence()})));
	let events = w.events().await;
	let (_, event) = events
		.iter()
		.find(|(kind, _)| kind == "run.call_declined")
		.unwrap();
	assert_eq!(event["by"], "erin");
}

#[rstest::rstest]
#[tokio::test]
async fn a_member_who_may_see_the_run_but_not_decline_is_refused_for_that_action_alone(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let bob = w.add_member_denied_decline("bob").await;
	w.drop_working_area().await;
	w.pause(true).await;
	let before = w.other_credential().await;
	w.set_credential(before).await;

	// Bob can read the Run, is not offered the call, and cannot decline it.
	let (status, page) = w.get(&bob).await;
	assert_eq!(status, 200, "{page}");
	assert!(page.get("declinable_call").is_none(), "{page}");
	let (status, body) = w.decline(&bob, &w.key()).await;
	assert_eq!(status, 403, "{body}");
	assert!(w.row().await.is_none());
	assert_eq!(w.management(&bob).await.1["control"], "PAUSED");
	assert_eq!(w.credential().await, before);

	// Lifting only that denial lets the same request through: it was `run.decline_call`.
	w.lift_the_denial().await;
	let (status, page) = w.get(&bob).await;
	assert_eq!(status, 200, "{page}");
	assert!(page["declinable_call"].is_object(), "{page}");
	let (status, declined) = w.decline(&bob, &w.key()).await;
	assert_eq!(status, 200, "{declined}");
	assert_eq!(w.credential().await, before);
}

#[rstest::rstest]
#[tokio::test]
async fn an_administrator_declines_a_run_with_a_working_area_though_they_cannot_read_it(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let erin = w.add_administrator("erin").await;
	let bob = w.add_member_denied_decline("bob").await;
	w.pause(true).await;
	let before = w.other_credential().await;
	w.set_credential(before).await;

	// A Working Area is owned by the Run's person, and `file.read` on it decides who may read
	// the Run (`run_area_readable`), so nobody else can read it.
	let (status, page) = w.get(&erin).await;
	assert_eq!(status, 403, "{page}");
	// The content-free management view offers the call, by tool and kind only, to someone who
	// may decline it, and to no one else.
	let (status, management) = w.management(&erin).await;
	assert_eq!(status, 200, "{management}");
	assert_eq!(
		management["declinable_call"],
		json!({"call": w.key(), "tool": "task_delegate", "kind": "policy"})
	);
	assert!(
		!management.to_string().contains("task.delegate"),
		"{management}"
	);
	let (status, management) = w.management(&bob).await;
	assert_eq!(status, 200, "{management}");
	assert!(management.get("declinable_call").is_none(), "{management}");
	let (status, body) = w.decline(&bob, &w.key()).await;
	assert_eq!(status, 403, "{body}");
	assert!(w.row().await.is_none());

	let (status, declined) = w.decline(&erin, &w.key()).await;
	assert_eq!(status, 200, "{declined}");
	assert_eq!(declined["control"], "ACTIVE");
	assert_eq!(w.credential().await, before);
	let (row, result) = w.row().await.expect("a declined call has a row");
	assert_eq!(row, "COMPLETED");
	assert_eq!(result, Some(json!({"error": sentence()})));
	let events = w.events().await;
	let (_, event) = events
		.iter()
		.find(|(kind, _)| kind == "run.call_declined")
		.unwrap();
	assert_eq!(event["by"], "erin");
	// Once the Run goes on, nothing is offered.
	let (_, management) = w.management(&erin).await;
	assert!(management.get("declinable_call").is_none(), "{management}");
}

#[rstest::rstest]
#[tokio::test]
async fn what_the_run_read_still_stops_someone_else_with_the_same_reason(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let erin = w.add_administrator("erin").await;
	w.pause(true).await;
	// The non-owner's walk is judged under erin's own policy, so both are kept from it.
	w.keep_definition_from(&["alice", "erin"]).await;
	let (status, resume) = w.control(&w.alice.clone(), "resume").await;
	assert_eq!(status, 409, "{resume}");
	// The block is judged on the Run's reads alone, so the Run's content stays hidden from erin
	// while the reason is the one the Run's person gets.
	let (status, body) = w.decline(&erin, &w.key()).await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["run_reads"], resume["run_reads"]);
	assert!(w.row().await.is_none());
	assert_eq!(w.management(&erin).await.1["control"], "PAUSED");
}
