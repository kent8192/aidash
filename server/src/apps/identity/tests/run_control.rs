//! A person's Run Control when an approval is withdrawn (#753, #769): a Run that only read the
//! version goes on, a Run whose Binding names it cannot be resumed, and `registry.read` alone hides.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_domain::registry::EntityRef;
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
	task: Uuid,
	workspace: Uuid,
	agent: aidash_server::registry::Entry,
	alice: String,
	_runtime: common::RuntimeFixture,
}

fn entity(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
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
		task: task.id,
		workspace: workspace.id,
		agent: registered[1].clone(),
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

	/// Sets the listed definition's approval on the Run's Node.
	async fn approve(&self, enabled: bool) {
		self.approve_entry(LISTED, enabled).await;
	}

	/// Sets the approval of `id@1.0.0` on the Run's Node.
	async fn approve_entry(&self, id: &str, enabled: bool) {
		let authorization = aidash_server::authorization::Authorization {
			pool: self.f.store.pool.clone(),
		};
		let node = common::default_node(&self.f.store, "acme").await;
		let current = authorization
			.catalog("acme", &node.node)
			.await
			.unwrap()
			.into_iter()
			.find(|binding| binding.entry_id == id)
			.unwrap();
		authorization
			.set_catalog(
				"acme",
				&node.node,
				&entity(id),
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

	/// `alice` may not read the listed definition.
	async fn keep_definition_from_alice(&self) {
		self.add_policies(
			None,
			vec![json!({"id":"no-definition","effect":"deny","subjects":{"ids":["alice"]},"actions":["registry.read"],"resources":{"kinds":["agent"],"ids":[LISTED]}})],
		)
		.await;
	}

	/// A second Run that produced this Run's Task and has read the listed definition, as the
	/// parent that delegated the Task has. The listed definition is no longer among this Run's
	/// own reads, so a walk meets it only while checking the Task.
	async fn add_parent_that_read_the_listed_definition(&self) {
		let acme = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
		let task = self
			.f
			.store
			.create_task(
				&acme,
				self.workspace,
				&aidash_server::domain::NewTask {
					title: "Parent".into(),
					description: "The Run that delegated the Task".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"human",
				None,
			)
			.await
			.unwrap();
		let parent = common::admitted_run(&self.f, "acme", &task, &self.agent).await;
		let pool = self.f.store.pool.fixture_driver();
		sqlx::query("DELETE FROM authorization_run_registry_reads WHERE run_id = $1")
			.bind(self.run)
			.execute(pool)
			.await
			.unwrap();
		sqlx::query(
			"INSERT INTO authorization_run_registry_reads (run_id, entry_id, entry_version, tenant) \
			 VALUES ($1, $2, $3, 'acme')",
		)
		.bind(parent.id)
		.bind(LISTED)
		.bind("1.0.0")
		.execute(pool)
		.await
		.unwrap();
		sqlx::query(
			"INSERT INTO authorization_run_outputs (run_id, tenant, workspace_id, resource_kind, resource_id) \
			 VALUES ($1, 'acme', $2, 'task', $3)",
		)
		.bind(parent.id)
		.bind(self.workspace)
		.bind(self.task)
		.execute(pool)
		.await
		.unwrap();
	}

	/// The Run has asked a person a question, as a Run waiting on a human has.
	async fn add_human_request(&self) {
		sqlx::query(
			"INSERT INTO human_requests (id, tenant, workspace_id, run_id, kind, prompt, request_key, created_at) \
			 VALUES ($1, 'acme', $2, $3, 'QUESTION', 'Which one?', 'ask', now())",
		)
		.bind(Uuid::new_v4())
		.bind(self.workspace)
		.bind(self.run)
		.execute(self.f.store.pool.fixture_driver())
		.await
		.unwrap();
	}

	/// `carol` may control Runs but not read them.
	async fn add_carol(&self) -> String {
		self.add_policies(
			Some("carol"),
			vec![json!({"id":"no-runs","effect":"deny","subjects":{"ids":["carol"]},"actions":["run.read"],"resources":{"kinds":["*"]}})],
		)
		.await;
		common::subject_credential(&self.f.store, "acme", "carol").await
	}
}

fn named(id: &str) -> Value {
	json!({"kind":"definition_not_approved","definition":{"id":id,"version":"1.0.0"}})
}

#[rstest::rstest]
#[tokio::test]
async fn a_withdrawal_leaves_a_run_that_only_listed_the_version_readable_and_resumable(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	// dev-lead found the version in `agent_discover` and nothing else (#769).
	let w = world(fixture).await;
	let alice = w.alice.clone();
	assert_eq!(w.get(&alice).await.0, 200);

	// The withdrawal names only what it stops: Runs using the version, not Runs that read it.
	let node = common::default_node(&w.f.store, "acme").await;
	let (status, impact) = common::request(
		&w.app,
		&alice,
		"GET",
		&format!(
			"/api/authorization/acme/catalog/impact?node={}&id={LISTED}&version=1.0.0",
			node.node
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{impact}");
	assert!(impact.get("reading_runs").is_none(), "{impact}");
	assert_eq!(impact["running_runs"], 0, "{impact}");

	w.approve(false).await;

	// The Run is still readable, and pausing and resuming it work.
	let (status, body) = w.get(&alice).await;
	assert_eq!(status, 200, "{body}");
	let (status, paused) = w.control(&alice, "pause").await;
	assert_eq!(status, 200, "{paused}");
	assert_eq!(paused["control"], "PAUSED");
	for hidden in ["context", "pending", "error", "agent_id"] {
		assert!(paused.get(hidden).is_none(), "{hidden} leaked: {paused}");
	}
	let (status, resumed) = w.control(&alice, "resume").await;
	assert_eq!(status, 200, "{resumed}");
	assert_eq!(resumed["control"], "ACTIVE");

	// Cancelling needs no approval either, and repeating it changes nothing.
	let (status, cancelled) = w.control(&alice, "cancel").await;
	assert_eq!(status, 200, "{cancelled}");
	assert_eq!(cancelled["control"], "CANCELLED");
	let (status, again) = w.control(&alice, "cancel").await;
	assert_eq!(status, 200, "{again}");
	assert_eq!(again["control"], "CANCELLED");
}

#[rstest::rstest]
#[tokio::test]
async fn resume_of_a_run_whose_binding_names_a_withdrawn_version_says_so_and_does_not_bounce(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	let (status, paused) = w.control(&alice, "pause").await;
	assert_eq!(status, 200, "{paused}");

	// The Run's own Agent is withdrawn: it can be read but not resumed.
	w.approve_entry("fixture-agent", false).await;
	assert_eq!(w.get(&alice).await.0, 200);
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["run_reads"], named("fixture-agent"));
	assert!(
		body["error"]
			.as_str()
			.unwrap()
			.contains("fixture-agent@1.0.0, which this Run uses, is no longer approved"),
		"{body}"
	);
	// It answered before waking the Run: still paused, never ACTIVE.
	assert_eq!(w.management(&alice).await.1["control"], "PAUSED");

	// Approving it again lets the Run go on.
	w.approve_entry("fixture-agent", true).await;
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["control"], "ACTIVE");
}

#[rstest::rstest]
#[tokio::test]
async fn a_definition_is_named_only_to_a_reader_of_it_and_policy_is_told_apart_from_approval(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();

	// Without `registry.read` the Run is hidden for the reader's own policy, approved or not.
	w.keep_definition_from_alice().await;
	for enabled in [true, false] {
		w.approve(enabled).await;
		let (status, body) = w.get(&alice).await;
		assert_eq!(status, 403, "{body}");
		assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
		assert!(!body.to_string().contains(LISTED), "{body}");
	}
	let (status, paused) = w.control(&alice, "pause").await;
	assert_eq!(status, 200, "{paused}");
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
	assert!(!body.to_string().contains(LISTED), "{body}");
}

#[rstest::rstest]
#[tokio::test]
async fn an_approved_agent_the_policy_refuses_is_not_usable_and_is_not_named_as_withdrawn(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.add_policies(
		None,
		vec![json!({"id":"no-execute","effect":"deny","subjects":{"ids":["alice"]},"actions":["agent.execute"],"resources":{"kinds":["agent"],"ids":["fixture-agent"]}})],
	)
	.await;
	let (status, paused) = w.control(&alice, "pause").await;
	assert_eq!(status, 200, "{paused}");
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["run_reads"], json!({"kind":"definition_not_usable"}));
	assert!(!body.to_string().contains("fixture-agent"), "{body}");
	assert_eq!(w.management(&alice).await.1["control"], "PAUSED");
}

#[rstest::rstest]
#[tokio::test]
async fn someone_who_may_not_read_the_run_can_stop_it_but_learns_nothing_of_its_reads(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let carol = w.add_carol().await;
	w.approve(false).await;
	let (status, body) = w.get(&carol).await;
	assert_eq!(status, 403, "{body}");
	assert!(body.get("run_reads").is_none(), "{body}");
	let (status, paused) = w.control(&carol, "pause").await;
	assert_eq!(status, 200, "{paused}");
	assert_eq!(paused["control"], "PAUSED");
	assert!(paused.get("context").is_none(), "{paused}");
	assert_eq!(w.management(&carol).await.1["control"], "PAUSED");
	let (status, body) = w.control(&carol, "resume").await;
	assert_eq!(status, 403, "{body}");
	assert!(body.get("run_reads").is_none(), "{body}");

	// Someone with no part in the Run may control it too, and is told nothing.
	w.add_policies(Some("bob"), vec![]).await;
	let bob = common::subject_credential(&w.f.store, "acme", "bob").await;
	let (status, body) = w.get(&bob).await;
	assert_eq!(status, 403, "{body}");
	assert!(body.get("run_reads").is_none(), "{body}");
	assert_eq!(w.management(&bob).await.1["control"], "PAUSED");
}

#[rstest::rstest]
#[case::memory("memory.read", "memory")]
#[case::human_request("human.read", "human_request")]
#[tokio::test]
async fn a_read_met_before_the_runs_own_policy_is_not_reported_to_someone_that_policy_hides_it_from(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
	#[case] action: &str,
	#[case] kind: &str,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.add_parent_that_read_the_listed_definition().await;
	w.add_human_request().await;
	w.keep_definition_from_alice().await;
	let (status, paused) = w.control(&alice, "pause").await;
	assert_eq!(status, 200, "{paused}");

	// The parent's read is checked with the Task, before the Run's own memory and human
	// requests, and it stops alice. Her policy allows the Run, so she is told what stopped it.
	let (status, body) = w.get(&alice).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));

	// Her own policy hides the Run as well, so approving the parent's read again would not
	// help her, and she is told nothing of what the parent read.
	w.add_policies(
		None,
		vec![json!({"id":"hidden-from-alice","effect":"deny","subjects":{"ids":["alice"]},"actions":[action],"resources":{"kinds":[kind]}})],
	)
	.await;
	let (status, body) = w.get(&alice).await;
	assert_eq!(status, 403, "{body}");
	assert!(body.get("run_reads").is_none(), "{body}");
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 403, "{body}");
	assert!(body.get("run_reads").is_none(), "{body}");
	// She can still stop it.
	assert_eq!(w.management(&alice).await.1["control"], "PAUSED");
}
