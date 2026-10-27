//! A Deployment Administrator's Safety Stop of one Run (#485, ADR 0101).
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::federation::Federation;
use serde_json::{Value, json};
use uuid::Uuid;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;

/// A Tenant with one local Run and one Run it executes for another Home.
struct World {
	f: Federation,
	app: common::TestApplication,
	run: Uuid,
	receiver: Uuid,
	subject: String,
	_runtime: common::RuntimeFixture,
}

async fn world(fixture: common::ApplicationFixture) -> World {
	let (f, _, _) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let subject = common::tenant_token(&f.store, "acme").await;
	common::tenant_root(&f, "other").await;
	let acme = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
	let workspace = f
		.store
		.create_workspace(&acme, "alice", "Safety Stop", "Stop a runaway Run")
		.await
		.unwrap();
	let new_task = |title: &str| aidash_server::domain::NewTask {
		title: title.into(),
		description: "A Run an administrator stops".into(),
		requirements: json!({}),
		dependencies: vec![],
		parent_id: None,
	};
	let task = f
		.store
		.create_task(&acme, workspace.id, &new_task("Local"), "human", None)
		.await
		.unwrap();
	let mut agent = None;
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
	] {
		agent = Some(
			f.registry
				.register(
					acme.tenant_name().unwrap(),
					serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"schema":{},"config":config})).unwrap(),
				)
				.await
				.unwrap(),
		);
	}
	// An admitted Run keeps its origin, so the Tenant can resume it afterwards.
	let run = common::admitted_run(&f, "acme", &task, &agent.unwrap()).await;
	let received = f
		.store
		.create_task(&acme, workspace.id, &new_task("Received"), "human", None)
		.await
		.unwrap();
	let receiver = common::remote_run_in(
		&f.store,
		"acme",
		&received,
		"aidash://home",
		"remote",
		"1.0.0",
	)
	.await;
	World {
		f,
		app,
		run: run.id,
		receiver: receiver.id,
		subject,
		_runtime: fixture.runtime,
	}
}

impl World {
	fn administrator(&self) -> &str {
		&self.f.config.api_token
	}

	async fn get(&self, token: &str, run: Uuid, query: &str) -> (u16, Value) {
		common::request(
			&self.app,
			token,
			"GET",
			&format!("/api/runs/{run}/management{query}"),
			Value::Null,
		)
		.await
	}

	async fn post(&self, token: &str, run: Uuid, key: Option<&str>, body: Value) -> (u16, Value) {
		let mut request = self
			.app
			.api_http
			.request(
				"POST".parse().unwrap(),
				&format!("/api/runs/{run}/management"),
			)
			.header("Authorization", format!("Bearer {token}"))
			.json(&body);
		if let Some(key) = key {
			request = request.header("Idempotency-Key", key);
		}
		let response = request.send().await.unwrap();
		let text = String::from_utf8_lossy(response.body()).into_owned();
		(
			response.status_code(),
			serde_json::from_str(&text).unwrap_or(Value::String(text)),
		)
	}

	async fn stop(&self, run: Uuid, key: Uuid, action: &str, revision: i64) -> (u16, Value) {
		self.post(
			self.administrator(),
			run,
			Some(&key.to_string()),
			json!({"tenant":"acme","action":action,"expected_control_revision":revision,"reason":"runaway spend"}),
		)
		.await
	}

	async fn facts(&self, run: Uuid) -> Value {
		let (status, body) = self.get(self.administrator(), run, "?tenant=acme").await;
		assert_eq!(status, 200, "{body}");
		body
	}

	async fn count(&self, sql: &str) -> i64 {
		sqlx::query_scalar(sql)
			.fetch_one(self.f.store.pool.fixture_driver())
			.await
			.unwrap()
	}

	async fn execute(&self, sql: &str) {
		sqlx::query(sql)
			.execute(self.f.store.pool.fixture_driver())
			.await
			.unwrap();
	}

	/// DDL, which only the schema owner may run.
	async fn alter(&self, sql: &str) {
		let owner = common::owner_pool(self.f.store.pool.fixture_driver()).await;
		sqlx::query(sql).execute(&owner).await.unwrap();
	}
}

#[rstest::rstest]
#[tokio::test]
async fn the_administrator_reads_only_the_allowlisted_metadata(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let facts = w.facts(w.run).await;
	let mut keys: Vec<_> = facts.as_object().unwrap().keys().cloned().collect();
	keys.sort();
	assert_eq!(
		keys,
		[
			"control",
			"control_revision",
			"control_source",
			"id",
			"phase"
		]
	);
	assert_eq!(facts["control"], "ACTIVE");
	assert_eq!(facts["control_revision"], 0);
	assert_eq!(facts["control_source"], "tenant");
	// A wrong Tenant and an unknown Run are the same refusal.
	let (wrong, wrong_body) = w.get(w.administrator(), w.run, "?tenant=other").await;
	let (unknown, unknown_body) = w
		.get(w.administrator(), Uuid::new_v4(), "?tenant=acme")
		.await;
	assert_eq!((wrong, unknown), (404, 404));
	assert_eq!(wrong_body, unknown_body);
	// The Tenant is required, and a Subject may not send it.
	assert_eq!(w.get(w.administrator(), w.run, "").await.0, 400);
	assert_eq!(w.get(&w.subject, w.run, "?tenant=acme").await.0, 400);
	w.f.store.pool.clone().close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_stop_pauses_replays_and_never_resumes(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let key = Uuid::new_v4();
	// Refused before any effect: wrong Tenant, bad reason, bad key.
	let other = w
		.post(
			w.administrator(),
			w.run,
			Some(&key.to_string()),
			json!({"tenant":"other","action":"pause","expected_control_revision":0,"reason":"x"}),
		)
		.await;
	assert_eq!(other.0, 404, "{}", other.1);
	for reason in ["   ", &"a".repeat(1001)] {
		let (status, body) = w
			.post(
				w.administrator(),
				w.run,
				Some(&key.to_string()),
				json!({"tenant":"acme","action":"pause","expected_control_revision":0,"reason":reason}),
			)
			.await;
		assert_eq!(status, 400, "{body}");
	}
	let bad_key = w
		.post(
			w.administrator(),
			w.run,
			Some("not-a-uuid"),
			json!({"tenant":"acme","action":"pause","expected_control_revision":0,"reason":"x"}),
		)
		.await;
	assert_eq!(bad_key.0, 400);
	assert_eq!(w.facts(w.run).await["control"], "ACTIVE");
	assert_eq!(w.count("SELECT count(*) FROM run_safety_stops").await, 0);

	// Accepted.
	let (status, receipt) = w.stop(w.run, key, "pause", 0).await;
	assert_eq!(status, 200, "{receipt}");
	assert_eq!(
		receipt,
		json!({"operation_id":key,"run_id":w.run,"tenant":"acme","action":"pause","outcome":"accepted"})
	);
	let facts = w.facts(w.run).await;
	assert_eq!(facts["control"], "PAUSED");
	assert_eq!(facts["control_revision"], 1);
	assert_eq!(facts["control_source"], "deployment_administrator");
	let (actor, identity, reason): (String, Option<Uuid>, String) = sqlx::query_as(
		"SELECT actor_kind, identity_id, reason FROM run_safety_stops WHERE operation_id = $1",
	)
	.bind(key)
	.fetch_one(w.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		(actor.as_str(), identity, reason.as_str()),
		("shared_bearer", None, "runaway spend")
	);

	// The Tenant resumes; the identical retry has no second effect.
	let (status, body) = common::request(
		&w.app,
		&w.subject,
		"POST",
		&format!("/api/runs/{}/control", w.run),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let facts = w.facts(w.run).await;
	assert_eq!(
		(
			facts["control"].clone(),
			facts["control_revision"].clone(),
			facts["control_source"].clone()
		),
		(json!("ACTIVE"), json!(2), json!("tenant"))
	);
	assert_eq!(w.stop(w.run, key, "pause", 0).await, (200, receipt.clone()));
	assert_eq!(w.facts(w.run).await["control"], "ACTIVE");
	assert_eq!(w.count("SELECT count(*) FROM run_safety_stops").await, 1);
	// The same key with other input conflicts; a stale revision conflicts.
	assert_eq!(w.stop(w.run, key, "cancel", 0).await.0, 409);
	assert_eq!(w.stop(w.run, Uuid::new_v4(), "pause", 1).await.0, 409);
	assert_eq!(w.count("SELECT count(*) FROM run_safety_stops").await, 1);

	// Worker progress does not make a stop stale.
	w.execute("UPDATE runs SET revision = revision + 5").await;
	let (status, body) = w.stop(w.run, Uuid::new_v4(), "pause", 2).await;
	assert_eq!(status, 200, "{body}");
	// Pausing a Run the Tenant paused is accepted; the source changes.
	w.execute(
		"UPDATE runs SET control = 'PAUSED', control_revision = 10, control_source = 'tenant'",
	)
	.await;
	let (status, body) = w.stop(w.run, Uuid::new_v4(), "pause", 10).await;
	assert_eq!(
		(status, body["outcome"].clone()),
		(200, json!("accepted")),
		"{body}"
	);
	let facts = w.facts(w.run).await;
	assert_eq!(
		(
			facts["control_revision"].clone(),
			facts["control_source"].clone()
		),
		(json!(11), json!("deployment_administrator"))
	);

	// Cancel, then nothing downgrades it, whatever the revision.
	let (status, body) = w.stop(w.run, Uuid::new_v4(), "cancel", 11).await;
	assert_eq!(
		(status, body["outcome"].clone()),
		(200, json!("accepted")),
		"{body}"
	);
	let (status, body) = w.stop(w.run, Uuid::new_v4(), "pause", 0).await;
	assert_eq!(
		(status, body["outcome"].clone()),
		(200, json!("no_op")),
		"{body}"
	);
	assert_eq!(w.facts(w.run).await["control"], "CANCELLED");
	let recorded = w
		.count("SELECT count(*) FROM events WHERE kind = 'run.control' AND data->>'source' = 'deployment_administrator'")
		.await;
	assert_eq!(
		recorded, 4,
		"each accepted stop appends one event naming its source"
	);
	assert_eq!(
		w.count("SELECT count(*) FROM events WHERE data::text LIKE '%runaway spend%'")
			.await,
		0,
		"the reason is in the audit record only"
	);
	w.f.store.pool.clone().close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn ordinary_control_refuses_every_administrator_action(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	for action in ["pause", "resume", "cancel"] {
		let (status, body) = common::request(
			&w.app,
			w.administrator(),
			"POST",
			&format!("/api/runs/{}/control", w.run),
			json!({"action":action}),
		)
		.await;
		assert_eq!(status, 403, "{action}: {body}");
		assert!(
			body.get("context").is_none() && body.get("state").is_none(),
			"{body}"
		);
	}
	assert_eq!(w.facts(w.run).await["control"], "ACTIVE");
	w.f.store.pool.clone().close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_subject_keeps_its_contract_and_may_not_send_administrator_fields(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	for extra in [
		json!({"action":"pause","reason":"x"}),
		json!({"action":"pause","tenant":"acme"}),
		json!({"action":"pause","expected_control_revision":0}),
	] {
		let (status, body) = w.post(&w.subject, w.run, None, extra).await;
		assert_eq!(status, 400, "{body}");
	}
	assert_eq!(w.facts(w.run).await["control"], "ACTIVE");
	let (status, body) = w
		.post(&w.subject, w.run, None, json!({"action":"pause"}))
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["control"], "PAUSED");
	let facts = w.facts(w.run).await;
	assert_eq!(facts["control_source"], "tenant");
	assert_eq!(w.count("SELECT count(*) FROM run_safety_stops").await, 0);
	// An administrator who sends the Subject body is refused.
	let (status, body) = w
		.post(
			w.administrator(),
			w.run,
			Some(&Uuid::new_v4().to_string()),
			json!({"action":"pause"}),
		)
		.await;
	assert_eq!(status, 400, "{body}");
	w.f.store.pool.clone().close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_identity_outage_pause_is_not_left_for_automatic_recovery(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	w.execute(
		"UPDATE runs SET control = 'PAUSED', error = 'identity status unavailable' WHERE id IN (SELECT id FROM runs ORDER BY id LIMIT 1)",
	)
	.await;
	let paused: Uuid =
		sqlx::query_scalar("SELECT id FROM runs WHERE error = 'identity status unavailable'")
			.fetch_one(w.f.store.pool.fixture_driver())
			.await
			.unwrap();
	// An unaware writer of `control` still moves the revision, as the system.
	let facts = w.facts(paused).await;
	assert_eq!(facts["control_revision"], 1);
	assert_eq!(facts["control_source"], "system");
	let (status, body) = w.stop(paused, Uuid::new_v4(), "pause", 1).await;
	assert_eq!(status, 200, "{body}");
	let marker: Option<String> = sqlx::query_scalar("SELECT error FROM runs WHERE id = $1")
		.bind(paused)
		.fetch_one(w.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		marker, None,
		"recovery resumes only rows that carry the outage marker"
	);
	w.f.store.pool.clone().close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_failed_audit_write_rolls_the_stop_back(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	// A real database fault on the audit insert, with no seam in production code.
	w.alter("CREATE FUNCTION fail_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'audit unavailable'; END $$").await;
	w.alter("CREATE TRIGGER fail_audit BEFORE INSERT ON run_safety_stops FOR EACH ROW EXECUTE FUNCTION fail_audit()").await;
	let key = Uuid::new_v4();
	let events = w.count("SELECT count(*) FROM events").await;
	let (status, body) = w.stop(w.run, key, "cancel", 0).await;
	assert!(status >= 500, "{status} {body}");
	assert_eq!(w.facts(w.run).await["control"], "ACTIVE");
	assert_eq!(w.facts(w.run).await["control_revision"], 0);
	assert_eq!(w.count("SELECT count(*) FROM events").await, events);
	assert_eq!(w.count("SELECT count(*) FROM run_safety_stops").await, 0);
	w.alter("DROP TRIGGER fail_audit ON run_safety_stops").await;
	let (status, body) = w.stop(w.run, key, "cancel", 0).await;
	assert_eq!(status, 200, "{body}");
	w.f.store.pool.clone().close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_receiver_run_is_stoppable_and_a_deleted_tenant_is_refused(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	// Required-Home receiver: content hidden, still stoppable here.
	let (status, body) = w.stop(w.receiver, Uuid::new_v4(), "pause", 0).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(w.facts(w.receiver).await["control"], "PAUSED");
	let key = Uuid::new_v4();
	let (status, receipt) = w.stop(w.run, key, "pause", 0).await;
	assert_eq!(status, 200, "{receipt}");
	// Tenant Deletion serializes with control: afterwards every command is
	// refused, a retry of an accepted one included.
	let (status, body) = common::request(
		&w.app,
		w.administrator(),
		"POST",
		"/api/tenants/acme/deletion",
		json!({"confirmation":"acme"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	for run in [w.run, w.receiver] {
		let (status, body) = w.stop(run, key, "pause", 0).await;
		assert_eq!(
			(status, body["error"].clone()),
			(410, json!("tenant_deleted")),
			"{body}"
		);
		let (status, _) = w.get(w.administrator(), run, "?tenant=acme").await;
		assert_eq!(status, 410);
	}
	let _ = TEST_FIXTURE;
	w.f.store.pool.clone().close().await;
}

/// A worker's retained authority transaction holds a key-share lock on its Run
/// through its foreign keys; a stop must not wait behind it.
#[rstest::rstest]
#[tokio::test]
async fn a_stop_does_not_wait_for_the_workers_own_run_references(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	use reinhardt::query::{
		Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
		QueryStatementBuilder as _,
	};
	let w = world(fixture).await;
	let mut authority = w.f.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(w.run)))
			.lock(LockType::KeyShare)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *authority)
	.await
	.unwrap();

	let stopped = tokio::time::timeout(
		std::time::Duration::from_secs(10),
		w.stop(w.run, Uuid::new_v4(), "pause", 0),
	)
	.await
	.expect("the stop must not wait for the authority transaction");

	authority.rollback().await.unwrap();
	assert_eq!(stopped.0, 200, "{}", stopped.1);
	w.f.store.pool.clone().close().await;
}

/// One operation ID sent for two Runs of a Tenant, at the same time, answers
/// the documented conflict for one of them and never a server error.
#[rstest::rstest]
#[tokio::test]
async fn one_operation_id_for_two_runs_conflicts_instead_of_failing(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let w = world(fixture).await;
	let key = Uuid::new_v4();
	let (first, second) = tokio::join!(
		w.stop(w.run, key, "pause", 0),
		w.stop(w.receiver, key, "pause", 0),
	);
	let mut statuses = [first.0, second.0];
	statuses.sort_unstable();
	assert_eq!(statuses, [200, 409], "{first:?} {second:?}");
	assert_eq!(w.count("SELECT count(*) FROM run_safety_stops").await, 1);
	w.f.store.pool.clone().close().await;
}
