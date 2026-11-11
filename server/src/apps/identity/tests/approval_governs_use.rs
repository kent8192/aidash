//! A Catalog Approval decides use, not who can read what Runs produced (#769, ADR 0122).
//!
//! Three Runs share a Workspace. `planner` runs `fixture-agent@1.0.0` and `reviewer` runs
//! `listed-agent@1.0.0`; `reviewer` read the `planner` Run, as a Run reads the output of the Run
//! it follows. `upgrader` runs `fixture-agent@2.0.0`. Withdrawing `fixture-agent@1.0.0` stops
//! only what uses it.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::registry::EntityRef;
use aidash_server::{authorization::identity::Actor, federation::Federation, harness::Harness};
use serde_json::{Value, json};
use uuid::Uuid;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;

const OLD: &str = "fixture-agent";

fn entity(id: &str, version: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: version.into(),
	}
}

struct World {
	f: Federation,
	app: common::TestApplication,
	planner: Uuid,
	reviewer: Uuid,
	upgrader: Uuid,
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
		.create_workspace(&acme, "alice", "Upgrade", "Replace an Agent version")
		.await
		.unwrap();
	let tenant = acme.tenant_name().unwrap();
	let mut registered = vec![];
	for (kind, id, version, config) in [
		(
			"model",
			"fixture-model",
			"1.0.0",
			json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:1/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
		),
		(
			"agent",
			OLD,
			"1.0.0",
			json!({"schema_version":1,"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Old","bindings":[],"remove_default":[]}),
		),
		(
			"agent",
			OLD,
			"2.0.0",
			json!({"schema_version":1,"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"New","bindings":[],"remove_default":[]}),
		),
		(
			"agent",
			"listed-agent",
			"1.0.0",
			json!({"schema_version":1,"model":{"id":"fixture-model","version":"1.0.0"},"instructions":"Reviews","bindings":[],"remove_default":[]}),
		),
	] {
		registered.push(
			f.registry
				.register(
					tenant,
					serde_json::from_value(json!({"id":id,"version":version,"kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"schema":{},"config":config})).unwrap(),
				)
				.await
				.unwrap(),
		);
	}
	let mut runs = vec![];
	for (title, agent) in [(1, 1), (2, 3), (3, 2)] {
		let task = f
			.store
			.create_task(
				&acme,
				workspace.id,
				&aidash_server::domain::NewTask {
					title: format!("Work {title}"),
					description: "A Run in the upgrade".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"human",
				None,
			)
			.await
			.unwrap();
		runs.push(common::admitted_run(&f, "acme", &task, &registered[agent]).await);
	}
	// `reviewer` read `planner`, as a Run reads the Run it follows.
	sqlx::query(
		"INSERT INTO authorization_run_reads (run_id, tenant, workspace_id, resource_kind, resource_id) \
		 VALUES ($1, 'acme', $2, 'run', $3)",
	)
	.bind(runs[1].id)
	.bind(workspace.id)
	.bind(runs[0].id)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	World {
		f,
		app,
		planner: runs[0].id,
		reviewer: runs[1].id,
		upgrader: runs[2].id,
		alice,
		_runtime: fixture.runtime,
	}
}

impl World {
	async fn get(&self, token: &str, run: Uuid) -> (u16, Value) {
		common::get(&self.app, token, &format!("/api/runs/{run}")).await
	}

	async fn control(&self, token: &str, run: Uuid, action: &str) -> (u16, Value) {
		common::post(
			&self.app,
			token,
			&format!("/api/runs/{run}/control"),
			json!({"action":action}),
		)
		.await
	}

	/// Sets the approval of `id@version` on the default Node.
	async fn approve(&self, id: &str, version: &str, enabled: bool) {
		let authorization = aidash_server::authorization::Authorization {
			pool: self.f.store.pool.clone(),
		};
		let node = common::default_node(&self.f.store, "acme").await;
		let current = authorization
			.catalog("acme", &node.node)
			.await
			.unwrap()
			.into_iter()
			.find(|binding| binding.entry_id == id && binding.entry_version == version)
			.unwrap();
		authorization
			.set_catalog(
				"acme",
				&node.node,
				&entity(id, version),
				current.revision,
				enabled,
				"deployment_administrator",
			)
			.await
			.unwrap();
	}

	async fn add_policies(&self, subject: Option<&str>, policies: Vec<Value>) {
		let current = aidash_server::authorization::Authorization {
			pool: self.f.store.pool.clone(),
		}
		.snapshot("acme")
		.await
		.unwrap();
		let mut bundle = serde_json::to_value(&current.bundle).unwrap();
		if let Some(subject) = subject {
			bundle["subjects"][subject] = json!({"kind":"user"});
		}
		bundle["policies"].as_array_mut().unwrap().extend(policies);
		common::replace_policy(&self.f.store, "acme", bundle).await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_withdrawal_stops_only_the_run_using_the_version_and_hides_nothing(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.approve(OLD, "1.0.0", false).await;

	// Every Run is still readable, the one that read the withdrawn version's Run as well.
	for run in [w.planner, w.reviewer, w.upgrader] {
		let (status, body) = w.get(&alice, run).await;
		assert_eq!(status, 200, "{body}");
	}

	// Only the Run using the version cannot be resumed.
	for (run, resumable) in [(w.planner, false), (w.reviewer, true), (w.upgrader, true)] {
		let (status, paused) = w.control(&alice, run, "pause").await;
		assert_eq!(status, 200, "{paused}");
		let (status, body) = w.control(&alice, run, "resume").await;
		if resumable {
			assert_eq!(status, 200, "{body}");
			assert_eq!(body["control"], "ACTIVE");
		} else {
			assert_eq!(status, 409, "{body}");
			assert_eq!(
				body["run_reads"],
				json!({"kind":"definition_not_approved","definition":{"id":OLD,"version":"1.0.0"}})
			);
		}
	}

	// Approving it again gives the Run back.
	w.approve(OLD, "1.0.0", true).await;
	let (status, body) = w.control(&alice, w.planner, "resume").await;
	assert_eq!(status, 200, "{body}");
}

#[rstest::rstest]
#[tokio::test]
async fn results_of_a_withdrawn_version_are_readable_only_with_registry_read(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.approve(OLD, "1.0.0", false).await;

	// A holder of `registry.read` reads the finished Run and the Run that read it.
	assert_eq!(w.get(&alice, w.planner).await.0, 200);
	assert_eq!(w.get(&alice, w.reviewer).await.0, 200);

	// Once she may no longer read the definition, she reads neither, and is told only the kind.
	w.add_policies(
		None,
		vec![json!({"id":"no-agent","effect":"deny","subjects":{"ids":["alice"]},"actions":["registry.read"],"resources":{"kinds":["agent"],"ids":[OLD]}})],
	)
	.await;
	for run in [w.planner, w.reviewer] {
		let (status, body) = w.get(&alice, run).await;
		assert_eq!(status, 403, "{body}");
		assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
		assert!(!body.to_string().contains(OLD), "{body}");
	}
}

#[rstest::rstest]
#[tokio::test]
async fn denying_registry_read_on_one_version_hides_its_outputs_and_stops_its_runs(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	// The claim in the withdrawal dialog: a deny on `registry.read` for the id and version.
	w.add_policies(
		None,
		vec![json!({
			"id":"hide-old","effect":"deny","subjects":{"ids":["alice"]},
			"actions":["registry.read"],"resources":{"kinds":["agent"],"ids":[OLD]},
			"condition":{"op":"eq","left":{"source":"resource","path":"/version"},"right":{"source":"literal","value":"1.0.0"}}
		})],
	)
	.await;

	// The version's Run and the Run that read it are hidden; another version is not.
	for run in [w.planner, w.reviewer] {
		let (status, body) = w.get(&alice, run).await;
		assert_eq!(status, 403, "{body}");
		assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
	}
	let (status, body) = w.get(&alice, w.upgrader).await;
	assert_eq!(status, 200, "{body}");

	// A hidden Run is stopped: it cannot be resumed, and the other version's Run can.
	for run in [w.planner, w.reviewer, w.upgrader] {
		let (status, paused) = w.control(&alice, run, "pause").await;
		assert_eq!(status, 200, "{paused}");
		let (status, body) = w.control(&alice, run, "resume").await;
		if run == w.upgrader {
			assert_eq!(status, 200, "{body}");
		} else {
			assert_eq!(status, 409, "{body}");
			assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
		}
	}
}

#[rstest::rstest]
#[tokio::test]
async fn narrowing_a_restriction_does_not_hide_outputs_but_still_refuses_a_start(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.add_policies(Some("carol"), vec![]).await;
	// Restrict the old version to `carol` alone: `alice` may no longer use it.
	let node = common::default_node(&w.f.store, "acme").await;
	let current = aidash_server::authorization::Authorization {
		pool: w.f.store.pool.clone(),
	}
	.catalog("acme", &node.node)
	.await
	.unwrap()
	.into_iter()
	.find(|binding| binding.entry_id == OLD && binding.entry_version == "1.0.0")
	.unwrap();
	let (status, body) = common::post(
		&w.app,
		&alice,
		"/api/authorization/acme/catalog",
		json!({"node":node.node,"entry":{"id":OLD,"version":"1.0.0"},"enabled":true,
			"expected_revision":current.revision,"restriction":{"subjects":["carol"],"groups":[]}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");

	// Existing outputs are still read by the people outside the restriction.
	assert_eq!(w.get(&alice, w.planner).await.0, 200);
	assert_eq!(w.get(&alice, w.reviewer).await.0, 200);

	// A new Run of the restricted version is still refused to them.
	let acme = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let workspace =
		w.f.store
			.create_workspace(&acme, "alice", "Another", "Start under a restriction")
			.await
			.unwrap();
	let task =
		w.f.store
			.create_task(
				&acme,
				workspace.id,
				&aidash_server::domain::NewTask {
					title: "Start".into(),
					description: "Refused".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"human",
				None,
			)
			.await
			.unwrap();
	let (status, body) = common::post(
		&w.app,
		&alice,
		&format!("/api/tasks/{}/claim", task.id),
		json!({"revision":task.revision,"agent":{"id":OLD,"version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 403, "{body}");
}

#[rstest::rstest]
#[tokio::test]
async fn a_worker_pauses_a_run_whose_binding_names_a_withdrawn_version_and_resume_does_not_bounce(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, task) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let approval = |enabled: bool, revision: i64| json!({"node":node,"entry":{"id":"model","version":"1.0.0"},"enabled":enabled,"expected_revision":revision});
	let set = async |body: Value| {
		common::post(&app, &token, "/api/authorization/acme/catalog", body)
			.await
			.0
	};
	assert_eq!(
		common::post(
			&app,
			&token,
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);

	// The model is in the Run's Binding: withdrawing it stops the Run at its next boundary.
	assert_eq!(set(approval(false, 1)).await, 200);
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let paused = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(paused.control.as_str(), "PAUSED", "{:?}", paused.error);
	let blocked = || async {
		f.store
			.events(&TEST_FIXTURE, 0, None, 1000)
			.await
			.unwrap()
			.iter()
			.filter(|event| {
				event.kind == "run.authorization_blocked" && event.data["run_id"] == json!(run.id)
			})
			.count()
	};
	assert_eq!(blocked().await, 1);

	// Resume says why and leaves the Run paused, so nothing wakes only to pause again.
	let (status, body) = common::post(
		&app,
		&token,
		&format!("/api/runs/{}/control", run.id),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(
		body["run_reads"],
		json!({"kind":"definition_not_approved","definition":{"id":"model","version":"1.0.0"}})
	);
	assert!(!harness.worker_once().await.unwrap());
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.control
			.as_str(),
		"PAUSED"
	);

	// Approved again, it resumes and the next boundary does not block it a second time.
	assert_eq!(set(approval(true, 2)).await, 200);
	let (status, body) = common::post(
		&app,
		&token,
		&format!("/api/runs/{}/control", run.id),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let _ = harness.worker_once().await;
	assert_eq!(blocked().await, 1, "resume must not bounce");
	common::cleanup(f, &url, &schema).await;
}

impl World {
	/// The Run's subject chain, as its execution grant saved it.
	async fn chain(&self, run: Uuid) -> Vec<String> {
		sqlx::query_scalar("SELECT subject_chain FROM authorization_execution WHERE run_id = $1")
			.bind(run)
			.fetch_one(self.f.store.pool.fixture_driver())
			.await
			.unwrap()
	}

	fn agent_subject(
		&self,
		node: &aidash_domain::nodes::NodeAddress,
		id: &str,
		version: &str,
	) -> String {
		aidash_server::domain::qualified_agent(node, id, version)
	}

	/// `run_for_action` under an `Access` built as the operation sweep builds it: the identity of
	/// the credential, then the subjects the operation saved.
	async fn action(&self, run: Uuid) -> Result<(), aidash_server::Error> {
		let authorization = aidash_server::authorization::Authorization {
			pool: self.f.store.pool.clone(),
		};
		let Actor::Subject(identity) = authorization.authenticate(&self.alice).await.unwrap()
		else {
			panic!("a Subject Credential authenticates a subject")
		};
		aidash_server::authorization::services::execution::operation_may_act(
			&self.f.store,
			&identity,
			self.chain(run).await,
			run,
		)
		.await
	}

	/// Restricts `id@version` on the default Node to the named subjects.
	async fn restrict(&self, id: &str, version: &str, subjects: Vec<String>) {
		let node = common::default_node(&self.f.store, "acme").await;
		let current = aidash_server::authorization::Authorization {
			pool: self.f.store.pool.clone(),
		}
		.catalog("acme", &node.node)
		.await
		.unwrap()
		.into_iter()
		.find(|binding| binding.entry_id == id && binding.entry_version == version)
		.unwrap();
		let (status, body) = common::post(
			&self.app,
			&self.alice,
			"/api/authorization/acme/catalog",
			json!({"node":node.node,"entry":{"id":id,"version":version},"enabled":true,
				"expected_revision":current.revision,"restriction":{"subjects":subjects,"groups":[]}}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
}

fn is_forbidden(result: Result<(), aidash_server::Error>) -> bool {
	matches!(result, Err(aidash_server::Error::Forbidden))
}

#[rstest::rstest]
#[tokio::test]
async fn a_running_operations_access_keeps_a_healthy_run_and_stops_one_that_cannot_use_its_binding(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let node = common::default_node(&w.f.store, "acme").await;
	let runs = [w.planner, w.reviewer, w.upgrader];

	// Healthy, with a dependency restricted to the Agents that use it (ADR 0095).
	for run in runs {
		w.action(run).await.unwrap();
	}
	let agents = vec![
		w.agent_subject(&node, OLD, "1.0.0"),
		w.agent_subject(&node, OLD, "2.0.0"),
		w.agent_subject(&node, "listed-agent", "1.0.0"),
	];
	w.restrict("fixture-model", "1.0.0", agents).await;
	for run in runs {
		w.action(run).await.unwrap();
	}

	// A withdrawn definition of the Binding stops every Run whose Binding names it.
	w.approve("fixture-model", "1.0.0", false).await;
	for run in runs {
		assert!(is_forbidden(w.action(run).await), "{run}");
	}
	w.approve("fixture-model", "1.0.0", true).await;

	// A withdrawn Agent stops only its own Runs, not the Run that read one of them.
	w.approve(OLD, "1.0.0", false).await;
	assert!(is_forbidden(w.action(w.planner).await));
	w.action(w.reviewer).await.unwrap();
	w.action(w.upgrader).await.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn resume_checks_the_run_as_its_own_subject_chain(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	let node = common::default_node(&w.f.store, "acme").await;
	let planner_agent = w.agent_subject(&node, OLD, "1.0.0");
	let reviewer_agent = w.agent_subject(&node, "listed-agent", "1.0.0");

	// A dependency restricted to the Agents that use it resumes for the person who owns the Run.
	w.restrict(
		"fixture-model",
		"1.0.0",
		vec![
			planner_agent.clone(),
			w.agent_subject(&node, OLD, "2.0.0"),
			reviewer_agent.clone(),
		],
	)
	.await;
	// `reviewer` is a child Run: root, the parent Agent, the child Agent. Its version is
	// restricted to the parent.
	sqlx::query("UPDATE authorization_execution SET subject_chain = $2 WHERE run_id = $1")
		.bind(w.reviewer)
		.bind(vec![
			"alice".to_owned(),
			planner_agent.clone(),
			reviewer_agent.clone(),
		])
		.execute(w.f.store.pool.fixture_driver())
		.await
		.unwrap();
	w.restrict("listed-agent", "1.0.0", vec![planner_agent])
		.await;
	for run in [w.planner, w.reviewer] {
		let (status, paused) = w.control(&alice, run, "pause").await;
		assert_eq!(status, 200, "{paused}");
		let (status, body) = w.control(&alice, run, "resume").await;
		assert_eq!(status, 200, "{run}: {body}");
		w.action(run).await.unwrap();
	}

	// Withdrawing either still answers `409`.
	for (run, id) in [(w.planner, "fixture-model"), (w.reviewer, "listed-agent")] {
		let (status, paused) = w.control(&alice, run, "pause").await;
		assert_eq!(status, 200, "{paused}");
		w.approve(id, "1.0.0", false).await;
		let (status, body) = w.control(&alice, run, "resume").await;
		assert_eq!(status, 409, "{id}: {body}");
		assert_eq!(
			body["run_reads"]["kind"], "definition_not_approved",
			"{body}"
		);
		w.approve(id, "1.0.0", true).await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_read_adds_to_the_retained_set_only_under_the_runs_own_authority(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let authorization = aidash_server::authorization::Authorization {
		pool: w.f.store.pool.clone(),
	};
	let Actor::Subject(identity) = authorization.authenticate(&w.alice).await.unwrap() else {
		panic!("a Subject Credential authenticates a subject")
	};
	let retained = async |inherited: bool| {
		aidash_server::authorization::services::execution::retained_after_read(
			&w.f.store,
			&identity,
			w.chain(w.reviewer).await,
			w.reviewer,
			inherited,
		)
		.await
		.unwrap()
	};
	let known = |set: &std::collections::BTreeSet<(String, String)>, id: &str, version: &str| {
		set.contains(&(id.to_owned(), version.to_owned()))
	};

	// Reading `reviewer` walks `planner` too: what that Run's Binding holds is remembered, as the
	// walk's approvals used to remember it, so a later admission under the lease still finds it.
	let own = retained(false).await;
	assert!(known(&own, OLD, "1.0.0"), "the other Run's Agent: {own:?}");
	assert!(known(&own, "fixture-model", "1.0.0"), "its model: {own:?}");
	assert!(!known(&own, OLD, "2.0.0"), "a Run that was not walked");

	// A read under an inherited lease must not enlarge it.
	let inherited = retained(true).await;
	assert!(inherited.is_empty(), "{inherited:?}");

	// Membership admits nothing: with the Agent withdrawn the admission still refuses.
	w.approve(OLD, "1.0.0", false).await;
	assert!(is_forbidden(w.action(w.planner).await));
}

#[rstest::rstest]
#[tokio::test]
async fn the_use_recheck_follows_the_node_the_run_executes_on(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let default = common::default_node(&f.store, "acme").await.node;
	let second = f
		.config
		.deployment
		.node(common::second_node_in(f.store.pool.fixture_driver(), "acme").await);
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let agent = f.registry.get(&tenant, "research", "1.0.0").await.unwrap();
	common::approve_agent_on(&f, "acme", &agent, &second).await;
	let (status, workspace) = common::post(
		&app,
		&token,
		"/api/workspaces",
		json!({"node":second.node,"title":"Second Node","goal":"Run elsewhere"}),
	)
	.await;
	assert_eq!(status, 200, "{workspace}");
	let (status, task) = common::post(
		&app,
		&token,
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Work","description":"On the second Node"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let (status, body) = common::post(
		&app,
		&token,
		&format!("/api/tasks/{}/claim", task["id"].as_str().unwrap()),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0).id;
	let set = async |node: &aidash_domain::nodes::NodeName, enabled: bool, revision: i64| {
		common::post(&app, &token, "/api/authorization/acme/catalog", json!({"node":node,"entry":{"id":"research","version":"1.0.0"},"enabled":enabled,"expected_revision":revision,"scope":"node"}))
		.await
		.0
	};
	let resume = async || {
		let (status, paused) = common::post(
			&app,
			&token,
			&format!("/api/runs/{run}/control"),
			json!({"action":"pause"}),
		)
		.await;
		assert_eq!(status, 200, "{paused}");
		common::post(
			&app,
			&token,
			&format!("/api/runs/{run}/control"),
			json!({"action":"resume"}),
		)
		.await
	};
	let action = async || {
		let authorization = aidash_server::authorization::Authorization {
			pool: f.store.pool.clone(),
		};
		let Actor::Subject(identity) = authorization.authenticate(&token).await.unwrap() else {
			panic!("a Subject Credential authenticates a subject")
		};
		let chain: Vec<String> = sqlx::query_scalar(
			"SELECT subject_chain FROM authorization_execution WHERE run_id = $1",
		)
		.bind(run)
		.fetch_one(f.store.pool.fixture_driver())
		.await
		.unwrap();
		aidash_server::authorization::services::execution::operation_may_act(
			&f.store, &identity, chain, run,
		)
		.await
	};

	// Withdrawn on the default Node, which is not the Run's: nothing is blocked.
	assert_eq!(set(&default, false, 1).await, 200);
	assert!(action().await.is_ok());
	let (status, body) = resume().await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(set(&default, true, 2).await, 200);

	// Withdrawn on the Run's own Node: blocked, for the sweep and for resume.
	assert_eq!(set(&second.node, false, 1).await, 200);
	assert!(is_forbidden(action().await));
	let (status, body) = resume().await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(
		body["run_reads"],
		json!({"kind":"definition_not_approved","definition":{"id":"research","version":"1.0.0"}})
	);
	common::cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_run_that_read_from_a_run_of_the_withdrawn_version_is_not_paused_at_its_boundary(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, first) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let research = f.registry.get(&tenant, "research", "1.0.0").await.unwrap();
	// A second Agent with its own Binding, so withdrawing `research` leaves it untouched.
	let other = f
		.registry
		.register(
			&tenant,
			serde_json::from_value(json!({"id":"other","version":"1.0.0","kind":"agent","name":{"en":"other"},"description":{"en":"Reads"},"schema":{},
				"config":{"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Read","bindings":[],"remove_default":["file_search","file_read"]}}))
			.unwrap(),
		)
		.await
		.unwrap();
	let first = f.store.task(&TEST_FIXTURE, first).await.unwrap();
	let workspace = first.workspace_id;
	let (status, second) = common::post(
		&app,
		&token,
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Reader","description":"Reads the first Run"}),
	)
	.await;
	assert_eq!(status, 200, "{second}");
	let second = f
		.store
		.task(
			&TEST_FIXTURE,
			second["id"].as_str().unwrap().parse().unwrap(),
		)
		.await
		.unwrap();
	let planner = common::admitted_run(&f, "acme", &first, &research).await;
	let reader = common::admitted_run(&f, "acme", &second, &other).await;
	sqlx::query(
		"INSERT INTO authorization_run_reads (run_id, tenant, workspace_id, resource_kind, resource_id) \
		 VALUES ($1, 'acme', $2, 'run', $3)",
	)
	.bind(reader.id)
	.bind(workspace)
	.bind(planner.id)
	.execute(f.store.pool.fixture_driver())
	.await
	.unwrap();
	let (status, body) = common::post(&app, &token, "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"research","version":"1.0.0"},"enabled":false,"expected_revision":1}))
	.await;
	assert_eq!(status, 200, "{body}");

	let harness = Harness {
		federation: f.clone(),
	};
	for _ in 0..8 {
		let _ = harness.worker_once().await;
	}
	let events = f.store.events(&TEST_FIXTURE, 0, None, 1000).await.unwrap();
	let blocked = |run: Uuid| {
		events
			.iter()
			.filter(|event| {
				event.kind == "run.authorization_blocked" && event.data["run_id"] == json!(run)
			})
			.count()
	};
	assert_eq!(
		blocked(planner.id),
		1,
		"the Run using the version is paused"
	);
	assert_eq!(
		blocked(reader.id),
		0,
		"the Run that only read from it is not"
	);
	common::cleanup(f, &url, &schema).await;
}
