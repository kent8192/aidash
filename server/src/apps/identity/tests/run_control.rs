//! A person's Run Control after an approval the Run has read is withdrawn (#753).
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
	alice: String,
	_runtime: common::RuntimeFixture,
}

fn listed() -> EntityRef {
	EntityRef {
		id: LISTED.into(),
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
		let authorization = aidash_server::authorization::Authorization {
			pool: self.f.store.pool.clone(),
		};
		let node = common::default_node(&self.f.store, "acme").await;
		let current = authorization
			.catalog("acme", &node.node)
			.await
			.unwrap()
			.into_iter()
			.find(|binding| binding.entry_id == LISTED)
			.unwrap();
		authorization
			.set_catalog(
				"acme",
				&node.node,
				&listed(),
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

fn named() -> Value {
	json!({"kind":"definition_not_approved","definition":{"id":LISTED,"version":"1.0.0"}})
}

#[rstest::rstest]
#[tokio::test]
async fn a_withdrawal_leaves_the_run_stoppable_but_hidden_until_it_is_approved_again(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	assert_eq!(w.get(&alice).await.0, 200);

	// The withdrawal shows who read the version before it happens.
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
	assert_eq!(impact["reading_runs"], 1, "{impact}");

	w.approve(false).await;

	// The Run is hidden, and its owner is told why.
	let (status, body) = w.get(&alice).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(body["error"], "forbidden");
	assert_eq!(body["run_reads"], named());

	// Pausing works and shows nothing the Run contains.
	let (status, paused) = w.control(&alice, "pause").await;
	assert_eq!(status, 200, "{paused}");
	assert_eq!(paused["control"], "PAUSED");
	assert_eq!(paused["control_source"], "tenant");
	for hidden in ["context", "pending", "error", "agent_id"] {
		assert!(paused.get(hidden).is_none(), "{hidden} leaked: {paused}");
	}

	// Resuming says what stands in the way and changes nothing.
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["run_reads"], named());
	assert!(
		body["error"]
			.as_str()
			.unwrap()
			.contains("listed-agent@1.0.0"),
		"{body}"
	);
	assert_eq!(w.management(&alice).await.1["control"], "PAUSED");

	// Approving it again gives the Run back, and it resumes.
	w.approve(true).await;
	assert_eq!(w.get(&alice).await.0, 200);
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["control"], "ACTIVE");

	// Cancelling needs no approval either, and repeating it changes nothing.
	w.approve(false).await;
	let (status, cancelled) = w.control(&alice, "cancel").await;
	assert_eq!(status, 200, "{cancelled}");
	assert_eq!(cancelled["control"], "CANCELLED");
	let (status, again) = w.control(&alice, "cancel").await;
	assert_eq!(status, 200, "{again}");
	assert_eq!(again["control"], "CANCELLED");
}

#[rstest::rstest]
#[tokio::test]
async fn a_reason_names_a_definition_only_to_a_reader_of_it(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let alice = w.alice.clone();
	w.keep_definition_from_alice().await;

	// While it is approved, only the reader's own policy stands in the way.
	let (status, body) = w.get(&alice).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(body["run_reads"], json!({"kind":"definition_not_readable"}));

	w.approve(false).await;
	let unnamed = json!({"kind":"definition_not_approved"});
	let (status, body) = w.get(&alice).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(body["run_reads"], unnamed);
	assert!(!body.to_string().contains(LISTED), "{body}");
	let (status, body) = w.control(&alice, "resume").await;
	assert_eq!(status, 409, "{body}");
	assert_eq!(body["run_reads"], unnamed);
	assert!(!body.to_string().contains(LISTED), "{body}");
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
