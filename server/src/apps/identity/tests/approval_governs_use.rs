//! A Catalog Approval decides use, not who can read what Runs produced (#769, ADR 0116).
//!
//! Three Runs share a Workspace. `planner` runs `fixture-agent@1.0.0` and `reviewer` runs
//! `listed-agent@1.0.0`; `reviewer` read the `planner` Run, as a Run reads the output of the Run
//! it follows. `upgrader` runs `fixture-agent@2.0.0`. Withdrawing `fixture-agent@1.0.0` stops
//! only what uses it.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_domain::registry::EntityRef;
use aidash_server::federation::Federation;
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
					tenant.clone(),
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
		common::request(
			&self.app,
			token,
			"GET",
			&format!("/api/runs/{run}"),
			Value::Null,
		)
		.await
	}

	async fn control(&self, token: &str, run: Uuid, action: &str) -> (u16, Value) {
		common::request(
			&self.app,
			token,
			"POST",
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

	/// A person who holds no grant on `fixture-agent` of any version.
	async fn dave(&self) -> String {
		self.add_policies(
			Some("dave"),
			vec![json!({"id":"dave-reads-runs","effect":"allow","subjects":{"ids":["dave"]},"actions":["workspace.read","run.read"],"resources":{"kinds":["*"]}}),
			json!({"id":"dave-no-agent","effect":"deny","subjects":{"ids":["dave"]},"actions":["registry.read"],"resources":{"kinds":["agent"],"ids":[OLD]}})],
		)
		.await;
		common::subject_credential(&self.f.store, "acme", "dave").await
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
	let dave = w.dave().await;
	w.approve(OLD, "1.0.0", false).await;

	// A holder of `registry.read` reads the finished Run and the Run that read it.
	assert_eq!(w.get(&alice, w.planner).await.0, 200);
	assert_eq!(w.get(&alice, w.reviewer).await.0, 200);

	// Someone without it reads neither, and is told only the kind.
	for run in [w.planner, w.reviewer] {
		let (status, body) = w.get(&dave, run).await;
		assert_eq!(status, 403, "{body}");
		assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
		assert!(!body.to_string().contains(OLD), "{body}");
	}
	// The unrelated Run is not affected.
	assert_eq!(w.get(&dave, w.upgrader).await.0, 200);
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
	let (status, body) = common::request(
		&w.app,
		&alice,
		"POST",
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
	let (status, body) = common::request(
		&w.app,
		&alice,
		"POST",
		&format!("/api/tasks/{}/claim", task.id),
		json!({"revision":task.revision,"agent":{"id":OLD,"version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 403, "{body}");
}
