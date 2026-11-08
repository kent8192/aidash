//! A Reference file's row lock against a Run's own outer transaction (#749).
//!
//! A step keeps one transaction open while a core tool call runs in an inner
//! one. If the outer one holds a lock the inner one needs in a conflicting mode,
//! the Run waits on itself and PostgreSQL cannot see the cycle. Each wait here
//! has a deadline, because the test database has no statement timeout and a
//! regression would otherwise hang the suite instead of failing it.
use super::deferred_exposure_tests::{Deferred, Reply, durable, events, scripted_core, settled};
use super::reference_set_tests::{add, agent_subject, bind_agent, file};
use super::*;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

/// One worker step. A healthy step takes seconds, even on a loaded host.
const STEP: Duration = Duration::from_secs(60);
/// The whole scripted Run, which is a few dozen steps.
const RUN: Duration = Duration::from_secs(300);
/// A person's request that must not queue behind a step.
const READ: Duration = Duration::from_secs(10);

async fn within<T>(
	limit: Duration,
	what: impl std::fmt::Display,
	future: impl std::future::Future<Output = T>,
) -> T {
	tokio::time::timeout(limit, future)
		.await
		.unwrap_or_else(|_| panic!("{what} did not answer within {limit:?}"))
}

/// Every tool call so far, oldest first, for a failure message.
fn tool_calls(run: &aidash_server::domain::Run) -> String {
	let context = durable(run);
	events(&context)
		.iter()
		.map(|(call, result)| format!("{} -> {result}", call["name"]))
		.collect::<Vec<_>>()
		.join("; ")
}

/// What a real Tenant's `members` group grants. The fixture's own policy allows
/// `*` to everyone, which would hide a missing grant. Neither the person nor the
/// Agents hold a `reference.*` action, as an Agent's subject does not in
/// production (ADR 0113). Both Agent versions join `members` explicitly;
/// without that the delegation to the second one is refused.
async fn narrow_to_members(c: &CoreFixture) {
	let mut policy = serde_json::to_value(
		aidash_domain::identity::tenancy::initial_policy("acme", false).unwrap(),
	)
	.unwrap();
	let agent = agent_subject(c, "1.2.0");
	policy["subjects"]["alice"] = json!({"kind":"user","groups":["members"]});
	let reader = aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(c.f.config.deployment.as_str(), "acme"),
		"reader",
		"1.0.0",
	);
	for subject in [&agent, &reader] {
		policy["subjects"][subject] = json!({
			"kind":"agent",
			"node":aidash_domain::nodes::NodeName::derived("acme"),
			"groups":["members"],
		});
	}
	// `agent_discover` needs `federation.discover`, which `members` does not
	// grant. This one allow is the only grant beyond what a real Tenant gives.
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"members-discover","effect":"allow",
		"subjects":{"groups":["members"]},
		"actions":["federation.discover"],
		"resources":{"kinds":["node"]},
	}));
	common::replace_policy(&c.f.store, "acme", policy).await;
}

async fn working_area(c: &CoreFixture, run: Uuid) -> Value {
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{run}/working-area"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	area
}

/// The Agent has no tool that copies a Reference file, so the copy is made over
/// HTTP between two steps, as a person would. Returns the Area's new revision.
async fn copy_first_reference_file(c: &CoreFixture, run: Uuid) -> i64 {
	let area = working_area(c, run).await;
	let (status, found) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{run}/files/search"),
		json!({"query":"","mode":"path","scope":"references"}),
	)
	.await;
	assert_eq!(status, 200, "{found}");
	let (status, copy) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{run}/files/materialize"),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"path":"copy.md","source":{"kind":"file","file_id":found["matches"][0]["file_id"],"expected_digest":found["matches"][0]["digest"]}}),
	)
	.await;
	assert_eq!(status, 200, "{copy}");
	let area = working_area(c, run).await;
	assert!(
		area["constraints"]
			.as_array()
			.unwrap()
			.iter()
			.any(|constraint| constraint["kind"] == "reference"),
		"the copy must lock the Area to its Reference file, or the later calls prove nothing: {area}"
	);
	area["revision"].as_i64().unwrap()
}

fn patch(revision: &AtomicI64, path: &str) -> Reply {
	Reply::Call(
		"apply_patch",
		json!({
			"idempotency_key":Uuid::new_v4(),
			"expected_revision":revision.load(Ordering::SeqCst),
			"preconditions":{path:null},
			"patch":format!("*** Begin Patch\n*** Add File: {path}\n+notes\n*** End Patch"),
		}),
	)
}

/// The call a stalled step was making, which `tool_calls` cannot list because
/// it has not completed.
fn pending_call(run: &aidash_server::domain::Run) -> String {
	run.state.tool().ok().map_or("none".to_owned(), |state| {
		state
			.response
			.tool_calls
			.iter()
			.map(|call| call.name.clone())
			.collect::<Vec<_>>()
			.join(", ")
	})
}

/// A second Agent bound to the same Source. A child Task cannot go to the
/// parent's own Agent (`SESSION_DEPENDENCY_CYCLE`), so delegating to this one is
/// what makes `pin` read the Reference row again under the parent's step.
async fn register_reader(c: &CoreFixture) {
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut agent =
		c.f.registry
			.get(&tenant, "research", "1.2.0")
			.await
			.unwrap();
	agent.id = "reader".into();
	agent.version = "1.0.0".into();
	agent.binding_normalization = None;
	agent.capabilities = vec!["reads.documents".into()];
	let (status, saved) = request(&c.app, &c.token, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{saved}");
	let (status, approved) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"reader","version":"1.0.0"},"expected_revision":0,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200, "{approved}");
}

#[rstest::rstest]
#[tokio::test]
async fn a_run_on_an_agent_with_a_source_completes_its_tool_calls_after_copying_a_file(
	#[future] scripted_core: Deferred,
) {
	let mut d = Box::pin(scripted_core).await;
	let token = d.c.token.clone();
	let (status, added) = add(&d.c, &token, vec![file("plan.md", "the plan\n")]).await;
	assert_eq!(status, 200, "{added}");
	let sources = [("project-docs", added["references"].clone())];
	bind_agent(&mut d.c, &sources).await;
	register_reader(&d.c).await;
	narrow_to_members(&d.c).await;
	let parent_task = d.c.task.to_string();
	let revision = Arc::new(AtomicI64::new(0));
	let copied = Arc::new(AtomicBool::new(false));
	let (revision_seen, copied_seen) = (revision.clone(), copied.clone());
	let run = d.delegate(d.c.task, "1.2.0").await;
	// The Run's own home Node, which `task_delegate` must name.
	let node = run.home_node.to_string();
	d.script(move |_, context| {
		// The delegated child Run is answered by the same provider.
		if context["current"]["task"]["id"] != parent_task.as_str() {
			return Reply::Text("Read the documents.");
		}
		let calls = events(context);
		let done = |name: &str| {
			calls
				.iter()
				.filter(|(call, _)| call["name"] == name)
				.count()
		};
		let result = |name: &str| {
			calls
				.iter()
				.find(|(call, _)| call["name"] == name)
				.map(|(_, result)| *result)
		};
		// Before the copy the step's outer transaction holds no lock on the file.
		if done("skill_list") == 0 {
			return Reply::Call("skill_list", json!({}));
		}
		if done("apply_patch") == 0 {
			return patch(&revision_seen, "before.md");
		}
		if !copied_seen.load(Ordering::SeqCst) {
			return Reply::Text("The copy was not made before the next model turn.");
		}
		// From here the outer transaction holds the file `FOR SHARE`.
		if done("skill_list") == 1 {
			return Reply::Call("skill_list", json!({}));
		}
		if done("apply_patch") == 1 {
			return patch(&revision_seen, "after.md");
		}
		let Some(child) = result("task_create") else {
			return Reply::Call(
				"task_create",
				json!({"title":"Child","description":"Read the plan","requirements":{}}),
			);
		};
		// `task_delegate` refuses an Agent this step has not discovered.
		if done("agent_discover") == 0 {
			return Reply::Call("agent_discover", json!({"capability":"reads.documents"}));
		}
		if done("task_delegate") == 0 {
			return Reply::Call(
				"task_delegate",
				json!({"task_id":child["id"],"node_id":node,"agent":{"id":"reader","version":"1.0.0"}}),
			);
		}
		Reply::Text("Done.")
	});
	revision.store(
		working_area(&d.c, run.id).await["revision"]
			.as_i64()
			.unwrap(),
		Ordering::SeqCst,
	);

	let deadline = tokio::time::Instant::now() + RUN;
	let finished = loop {
		if tokio::time::Instant::now() > deadline {
			let run = d.run(run.id).await;
			panic!(
				"the Run did not settle within {RUN:?}: phase={} state={:?} error={:?} completed tool calls: [{}]",
				run.phase(),
				run.state,
				run.error,
				tool_calls(&run)
			);
		}
		if tokio::time::timeout(STEP, d.step()).await.is_err() {
			let run = d.run(run.id).await;
			panic!(
				"a worker step did not finish within {STEP:?}: phase={} calling={} error={:?} completed tool calls: [{}]",
				run.phase(),
				pending_call(&run),
				run.error,
				tool_calls(&run)
			);
		}
		let now = d.run(run.id).await;
		if settled(&now) {
			break now;
		}
		if !copied.load(Ordering::SeqCst) && events(&durable(&now)).len() == 2 {
			let after = copy_first_reference_file(&d.c, run.id).await;
			// The patch before the copy moved the revision; the copy moved it again.
			revision.store(after, Ordering::SeqCst);
			copied.store(true, Ordering::SeqCst);
		}
	};

	assert_eq!(
		finished.phase(),
		aidash_server::domain::RunPhase::Completed,
		"error={:?} completed tool calls: [{}]",
		finished.error,
		tool_calls(&finished)
	);
	let context = durable(&finished);
	let names = events(&context)
		.iter()
		.map(|(call, _)| call["name"].as_str().unwrap().to_owned())
		.collect::<Vec<_>>();
	assert_eq!(
		names,
		[
			"skill_list",
			"apply_patch",
			"skill_list",
			"apply_patch",
			"task_create",
			"agent_discover",
			"task_delegate"
		],
		"{context}"
	);
	for (call, result) in events(&context) {
		assert!(
			result.get("error").is_none() && result.get("refused").is_none(),
			"{} failed: {result}",
			call["name"]
		);
	}
	let runs = d.c.f.store.runs(&TEST_FIXTURE).await.unwrap();
	let child = runs.iter().find(|child| child.id != finished.id).unwrap();
	assert_eq!(
		child.phase(),
		aidash_server::domain::RunPhase::Completed,
		"the delegated Run must pin the same file and finish: {:?}",
		child.error
	);
	d.close().await;
}

/// Stands in for a step that has copied the file: its outer transaction holds
/// the Reference row `FOR SHARE` until it ends.
async fn hold_shared(c: &CoreFixture, id: &str) -> sqlx::Transaction<'static, sqlx::Postgres> {
	let mut holder = c.f.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query("SELECT 1 FROM core_records WHERE id = $1::uuid AND kind = 'reference' FOR SHARE")
		.bind(id)
		.execute(&mut *holder)
		.await
		.unwrap();
	holder
}

#[rstest::rstest]
#[tokio::test]
async fn a_persons_read_of_a_file_does_not_wait_for_a_step_that_holds_a_copy(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let (status, added) = add(&c, &c.token, vec![file("plan.md", "the plan\n")]).await;
	assert_eq!(status, 200, "{added}");
	let id = added["files"][0]["reference"]["reference_id"]
		.as_str()
		.unwrap();
	let holder = hold_shared(&c, id).await;

	for path in [
		format!("/api/references/{id}"),
		"/api/references".to_owned(),
		format!("/api/references/{id}/download"),
	] {
		let (status, body) = within(
			READ,
			format!("GET {path} behind a shared lock"),
			request(&c.app, &c.token, "GET", &path, Value::Null),
		)
		.await;
		assert_eq!(status, 200, "{path}: {body}");
	}

	holder.rollback().await.unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deleting_a_file_a_step_has_copied_waits_for_that_step(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let (status, added) = add(&c, &c.token, vec![file("plan.md", "the plan\n")]).await;
	assert_eq!(status, 200, "{added}");
	let reference = &added["files"][0]["reference"];
	let id = reference["reference_id"].as_str().unwrap();
	let holder = hold_shared(&c, id).await;
	let path = format!("/api/references/{id}/revoke");
	let mut revoke = Box::pin(request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"expected_revision":reference["revision"]}),
	));

	let early = tokio::time::timeout(Duration::from_secs(1), &mut revoke).await;

	assert!(
		early.is_err(),
		"revocation must wait for the step that holds a copy: {early:?}"
	);
	holder.rollback().await.unwrap();
	let (status, revoked) = within(READ, "revocation after the step ended", revoke).await;
	assert_eq!(status, 200, "{revoked}");
	c.close().await;
}
