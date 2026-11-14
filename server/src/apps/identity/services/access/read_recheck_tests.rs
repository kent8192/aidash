//! Read checks cost the same however much a Run has read, and a repeated allow
//! adds no Authorization Decision (ADR 0116).
use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::authorization::Authorization;
use crate::egress_tests::statements::recorded;
use aidash_application::tenancy::TenancyScope;
use aidash_domain::identity::execution::ExecutionGrant;
use rstest::rstest;
use std::time::Duration;

const DEPLOYMENT: &str = "aidash://origin-fixture";

struct Fixture {
	_database: DatabaseFixture,
	store: Store,
	identity: SubjectIdentity,
	workspace: Uuid,
	task: Uuid,
	run: Uuid,
}

/// Everything is allowed, except `deny` when given, as `(subject, action)`.
fn policy(deny: Option<(&str, &str)>) -> Value {
	let mut policies = vec![json!({
		"id":"everything", "effect":"allow", "subjects":{"any":true},
		"actions":["*"], "resources":{"kinds":["*"]}
	})];
	if let Some((subject, action)) = deny {
		policies.push(json!({
			"id":"revoked", "effect":"deny", "subjects":{"ids":[subject]},
			"actions":[action], "resources":{"kinds":["*"]}
		}));
	}
	json!({
		"tenant":"acme",
		"subjects":{"reader":{"kind":"user"},"helper":{"kind":"user"},"viewer":{"kind":"user"}},
		"policies":policies
	})
}

impl Fixture {
	fn driver(&self) -> &sqlx::PgPool {
		self.store.pool.fixture_driver()
	}

	async fn set_policy(&self, expected: i64, deny: Option<(&str, &str)>) {
		Authorization {
			pool: self.store.pool.clone(),
		}
		.replace(
			"acme",
			expected,
			serde_json::from_value(policy(deny)).unwrap(),
			"deployment_administrator",
		)
		.await
		.unwrap();
	}

	async fn access(&self) -> Access {
		Access::begin(&self.store, &self.identity).await.unwrap()
	}

	/// A Read Recheck under the Run's two-subject chain, as the 30 second loop runs it.
	async fn recheck(&self) -> Result<()> {
		let mut access = self.access().await;
		access.subjects = vec!["reader".into(), "helper".into()];
		let result = access.recheck_run_reads(self.run).await.map(|_| ());
		access.finish(result).await
	}

	/// A person opening the Run's output.
	async fn open(&self) -> Result<()> {
		let mut access = self.access().await;
		let result = access.run_for_interaction(self.run).await.map(|_| ());
		access.finish(result).await
	}

	async fn rows(&self, filter: &str) -> i64 {
		sqlx::query_scalar(&format!(
			"SELECT count(*) FROM authorization_decisions WHERE {filter}"
		))
		.fetch_one(self.driver())
		.await
		.unwrap()
	}

	/// Holds this Tenant's audit lock until the transaction ends.
	async fn hold_audit_lock(&self, tenant: &str) -> sqlx::Transaction<'static, sqlx::Postgres> {
		let mut tx = self.driver().begin().await.unwrap();
		sqlx::query("SELECT pg_advisory_xact_lock(71003202, hashtext($1))")
			.bind(tenant)
			.execute(&mut *tx)
			.await
			.unwrap();
		tx
	}
}

/// A Run that read `reads` Messages and one of each other kind of record.
async fn fixture(database: DatabaseFixture, reads: usize) -> Fixture {
	let store = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		aidash_domain::nodes::DeploymentId::parse(DEPLOYMENT).unwrap(),
	)
	.await
	.unwrap();
	let driver = store.pool.fixture_driver().clone();
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	sqlx::query("INSERT INTO tenants (name,personal,created_at,created_by_kind) VALUES ('acme',false,clock_timestamp(),'deployment_administrator')").execute(&driver).await.unwrap();
	sqlx::query(
		"INSERT INTO node_names (name,tenant,created_at) VALUES ($1,'acme',clock_timestamp())",
	)
	.bind(&node)
	.execute(&driver)
	.await
	.unwrap();
	sqlx::query("INSERT INTO nodes (name,tenant,label,is_default,created_at,created_by_kind) VALUES ($1,'acme','Default',true,clock_timestamp(),'deployment_administrator')").bind(&node).execute(&driver).await.unwrap();
	Authorization {
		pool: store.pool.clone(),
	}
	.replace(
		"acme",
		0,
		serde_json::from_value(policy(None)).unwrap(),
		"deployment_administrator",
	)
	.await
	.unwrap();
	let workspace = Uuid::new_v4();
	let scratch = crate::apps::execution::test_database::fixture_scratch_space(
		&driver, "acme", &node, "reader",
	)
	.await;
	sqlx::query("INSERT INTO workspaces (id,tenant,node,title,goal,state,revision,scratch_space_id,created_by) VALUES ($1,'acme',$2,'Fixture','Fixture','{}',1,$3,'reader')").bind(workspace).bind(&node).bind(scratch).execute(&driver).await.unwrap();
	sqlx::query("INSERT INTO authorization_workspaces (workspace_id,tenant,owner_subject) VALUES ($1,'acme','reader')").bind(workspace).execute(&driver).await.unwrap();
	let credential = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_credentials (id,tenant,subject,token_hash,created_at,expires_at,issued_by) VALUES ($1,'acme','reader',$2,clock_timestamp(),clock_timestamp()+interval '1 day','fixture')").bind(credential).bind(vec![1u8;32]).execute(&driver).await.unwrap();
	let task = Uuid::new_v4();
	let read_task = Uuid::new_v4();
	for id in [task, read_task] {
		sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,'acme',$2,'Fixture','Fixture','COMPLETED',1,'reader')").bind(id).bind(workspace).execute(&driver).await.unwrap();
	}
	let run = Uuid::new_v4();
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	sqlx::query("INSERT INTO runs (id,tenant,task_id,workspace_id,node,home_node,agent_id,agent_version,phase,pending,revision) VALUES ($1,'acme',$2,$3,$4,$5,'fixture','1.0.0','COMPLETED',$6,1)").bind(run).bind(task).bind(workspace).bind(&node).bind(DEPLOYMENT).bind(pending).execute(&driver).await.unwrap();
	let artifact = Uuid::new_v4();
	sqlx::query("INSERT INTO artifacts (id,tenant,workspace_id,task_id,kind,name,content,created_by,idempotency_key) VALUES ($1,'acme',$2,$3,'json','Fixture','{}','reader',$1::text)").bind(artifact).bind(workspace).bind(read_task).execute(&driver).await.unwrap();
	let mut sources = vec![
		("task".to_owned(), read_task),
		("artifact".to_owned(), artifact),
	];
	for _ in 0..reads {
		let message = Uuid::new_v4();
		sqlx::query("INSERT INTO messages (id,tenant,workspace_id,sender,content) VALUES ($1,'acme',$2,'reader','Fixture')").bind(message).bind(workspace).execute(&driver).await.unwrap();
		sources.push(("message".to_owned(), message));
	}
	let mut tx = store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	crate::apps::identity::repositories::admission::persist_grant_in(
		&mut *tx,
		&ExecutionGrant {
			run_id: run,
			task_id: task,
			workspace_id: workspace,
			tenant: "acme".into(),
			credential_id: credential,
			root_subject: "reader".into(),
			subject_chain: vec!["reader".into(), "helper".into()],
		},
	)
	.await
	.unwrap();
	crate::apps::identity::models::AuthorizationRunRead::record_sources(
		&mut *tx, run, workspace, &sources,
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	Fixture {
		_database: database,
		store,
		identity: SubjectIdentity {
			http_session: None,
			credential_id: credential,
			tenant: "acme".into(),
			subject: "reader".into(),
		},
		workspace,
		task,
		run,
	}
}

#[rstest]
#[case::few(10)]
#[case::many(200)]
#[tokio::test]
async fn a_repeated_recheck_writes_nothing_and_takes_no_audit_lock(
	#[future] database: DatabaseFixture,
	#[case] reads: usize,
) {
	// Arrange
	let fixture = fixture(database.await, reads).await;
	fixture.recheck().await.unwrap();
	let written = fixture.rows("coalesced").await;
	assert!(
		written >= 2 * reads as i64,
		"each subject's read of each Message is recorded once: {written}"
	);
	assert_eq!(fixture.rows("NOT coalesced").await, 0);
	let held = fixture.hold_audit_lock("acme").await;

	// Act: the audit lock is taken, so this would wait if it wrote or locked.
	let repeat = tokio::time::timeout(Duration::from_secs(30), fixture.recheck()).await;

	// Assert
	repeat.expect("no audit lock is needed").unwrap();
	held.rollback().await.unwrap();
	assert_eq!(fixture.rows("coalesced").await, written);
}

#[rstest]
#[case::read_recheck(false)]
#[case::person(true)]
#[tokio::test]
async fn the_statement_count_does_not_grow_with_the_reads(
	#[future] database: DatabaseFixture,
	#[case] person: bool,
) {
	// Arrange
	let fixture = fixture(database.await, 10).await;
	let check = async |fixture: &Fixture| {
		let mut access = fixture.access().await;
		if !person {
			access.subjects = vec!["reader".into(), "helper".into()];
		}
		let (result, statements) = recorded(async {
			let result = if person {
				access.run_for_interaction(fixture.run).await
			} else {
				access.recheck_run_reads(fixture.run).await
			};
			result.map(|_| ())
		})
		.await;
		result.unwrap();
		access.finish(Ok(())).await.unwrap();
		statements
	};
	// The first check writes its rows, in batches, and later ones do not.
	check(&fixture).await;
	let few = check(&fixture).await;

	// Act: the same Run with 190 more recorded reads.
	let mut sources = vec![];
	for _ in 0..190 {
		let message = Uuid::new_v4();
		sqlx::query("INSERT INTO messages (id,tenant,workspace_id,sender,content) VALUES ($1,'acme',$2,'reader','Fixture')").bind(message).bind(fixture.workspace).execute(fixture.driver()).await.unwrap();
		sources.push(("message".to_owned(), message));
	}
	let mut tx = fixture
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	crate::apps::identity::models::AuthorizationRunRead::record_sources(
		&mut *tx,
		fixture.run,
		fixture.workspace,
		&sources,
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	check(&fixture).await;
	let many = check(&fixture).await;

	// Assert
	assert!(!few.is_empty(), "the counter sees statements");
	assert_eq!(few.len(), many.len(), "{many:#?}");
	assert!(many.len() <= 60, "{} statements: {many:#?}", many.len());
}

#[rstest]
#[tokio::test]
async fn a_policy_change_records_once_more(#[future] database: DatabaseFixture) {
	// Arrange
	let fixture = fixture(database.await, 5).await;
	fixture.recheck().await.unwrap();
	let first = fixture.rows("coalesced").await;

	// Act
	fixture.set_policy(1, None).await;
	fixture.recheck().await.unwrap();
	fixture.recheck().await.unwrap();

	// Assert
	assert_eq!(fixture.rows("coalesced").await, first * 2);
	assert_eq!(fixture.rows("coalesced AND revision = 2").await, first);
}

#[rstest]
#[tokio::test]
async fn a_revoked_read_is_refused_and_each_denial_is_recorded(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let fixture = fixture(database.await, 5).await;
	fixture.recheck().await.unwrap();

	// Act
	fixture
		.set_policy(1, Some(("reader", "message.read")))
		.await;
	let first = fixture.recheck().await;
	let second = fixture.recheck().await;

	// Assert
	assert!(matches!(first, Err(Error::Forbidden)), "{first:?}");
	assert!(matches!(second, Err(Error::Forbidden)), "{second:?}");
	assert_eq!(
		fixture
			.rows(
				"NOT (decision->>'allowed')::boolean AND action = 'message.read' AND NOT coalesced"
			)
			.await,
		// The walk stops at the first denied Message, in each refused request.
		2
	);
}

#[rstest]
#[tokio::test]
async fn a_person_opening_a_run_twice_records_what_they_opened_both_times(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let fixture = fixture(database.await, 5).await;

	// Act
	fixture.open().await.unwrap();
	let dependencies = fixture.rows("coalesced").await;
	let opened = fixture.rows("NOT coalesced").await;
	fixture.open().await.unwrap();

	// Assert
	assert!(
		dependencies >= 5,
		"the Messages the Run read: {dependencies}"
	);
	assert!(opened > 0, "what the person opened");
	assert_eq!(fixture.rows("coalesced").await, dependencies);
	assert_eq!(fixture.rows("NOT coalesced").await, opened * 2);
	assert_eq!(
		fixture
			.rows(&format!("coalesced AND run_id = '{}'", fixture.run))
			.await,
		dependencies,
		"a dependency row names the Run whose output was opened"
	);
	assert_eq!(
		fixture.rows("NOT coalesced AND run_id IS NOT NULL").await,
		0
	);
}

#[rstest]
#[tokio::test]
async fn one_request_records_a_hidden_item_once(#[future] database: DatabaseFixture) {
	// Arrange
	let fixture = fixture(database.await, 1).await;
	fixture.set_policy(1, Some(("reader", "note.read"))).await;
	let mut access = fixture.access().await;
	let resource = access.resource("note", "hidden", json!({}));

	// Act: a list that filters the same item out twice.
	assert!(!access.decide(&resource, "note.read").await.unwrap());
	assert!(!access.decide(&resource, "note.read").await.unwrap());
	access.finish(Ok(())).await.unwrap();

	// Assert
	assert_eq!(fixture.rows("action = 'note.read'").await, 1);
}

#[rstest]
#[tokio::test]
async fn a_worker_records_its_reads_once_and_every_call_it_starts(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let fixture = fixture(database.await, 1).await;
	let mut access = fixture.access().await;
	access.read_run = Some(fixture.run);
	access.durable_audit = true;
	let message = access.resource("message", Uuid::new_v4(), json!({}));
	let tool = access.resource("tool", "search", json!({}));

	// Act: the same read in two steps, around two tool calls.
	for _ in 0..2 {
		assert!(access.decide(&message, "message.read").await.unwrap());
		assert!(access.decide(&tool, "tool.invoke").await.unwrap());
	}

	// Assert: each row committed before the effect that the tool call starts.
	assert_eq!(
		fixture.rows("action = 'message.read' AND coalesced").await,
		1
	);
	assert_eq!(
		fixture
			.rows(&format!(
				"action = 'tool.invoke' AND NOT coalesced AND run_id = '{}'",
				fixture.run
			))
			.await,
		2
	);
	access.finish(Ok(())).await.unwrap();
	assert_eq!(fixture.rows("action = 'message.read'").await, 1);
}

#[rstest]
#[tokio::test]
async fn two_simultaneous_checks_write_each_row_once(#[future] database: DatabaseFixture) {
	// Arrange: both find the row missing before either writes it.
	let fixture = fixture(database.await, 1).await;
	let mut first = fixture.access().await;
	let mut second = fixture.access().await;
	let resource = first.resource("message", "shared", json!({}));
	let row = DecisionRow {
		input: first.evaluation("reader", &resource, "message.read"),
		decision: Decision {
			allowed: true,
			reason: "allow".into(),
			matched_policies: vec![],
			effective_roles: Default::default(),
			revision: first.snapshot.revision,
		},
		owner: Some(DecisionOwner::Run(fixture.run)),
		coalesced: true,
	};
	let first_rows = AuthorizationDecision::unrecorded(&mut **first.tx, "acme", vec![row.clone()])
		.await
		.unwrap();
	let second_rows = AuthorizationDecision::unrecorded(&mut **second.tx, "acme", vec![row])
		.await
		.unwrap();
	assert_eq!((first_rows.len(), second_rows.len()), (1, 1));

	// Act
	AuthorizationDecision::append_rows(&mut **first.tx, "acme", &first_rows)
		.await
		.unwrap();
	first.finish(Ok(())).await.unwrap();
	AuthorizationDecision::append_rows(&mut **second.tx, "acme", &second_rows)
		.await
		.unwrap();
	second.finish(Ok(())).await.unwrap();

	// Assert
	assert_eq!(fixture.rows("coalesced").await, 1);
}

#[rstest]
#[tokio::test]
async fn other_tenants_do_not_wait_for_a_tenants_audit_writer(#[future] database: DatabaseFixture) {
	// Arrange
	let fixture = fixture(database.await, 1).await;
	let held = fixture.hold_audit_lock("another-tenant").await;

	// Act: the writer's own Tenant is free.
	let free = tokio::time::timeout(Duration::from_secs(30), fixture.recheck()).await;
	held.rollback().await.unwrap();

	// Assert
	free.expect("another Tenant's lock does not block this one")
		.unwrap();

	// Arrange: the same Tenant still waits, so a cursor never skips a writer.
	let held = fixture.hold_audit_lock("acme").await;
	fixture.set_policy(1, None).await;

	// Act
	let blocked = tokio::time::timeout(Duration::from_millis(500), fixture.recheck()).await;

	// Assert
	assert!(blocked.is_err(), "a new row waits for the Tenant's lock");
	held.rollback().await.unwrap();
}

#[rstest]
#[tokio::test]
async fn the_continuation_check_never_reads_what_the_run_has_read(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let fixture = fixture(database.await, 20).await;
	let mut access = fixture.access().await;

	// Act
	let (run, statements) = recorded(access.run_for_continuation(fixture.run)).await;

	// Assert
	assert_eq!(run.unwrap().task_id, fixture.task);
	assert_eq!(access.context["workspace_id"], json!(fixture.workspace));
	assert!(
		statements
			.iter()
			.all(|statement| !statement.contains("authorization_run_reads")),
		"{statements:#?}"
	);
	access.finish(Ok(())).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn the_decision_page_names_the_run(#[future] database: DatabaseFixture) {
	// Arrange
	let fixture = fixture(database.await, 1).await;
	fixture.recheck().await.unwrap();

	// Act
	let page = Authorization {
		pool: fixture.store.pool.clone(),
	}
	.decisions("acme", 0, 5)
	.await
	.unwrap();

	// Assert
	assert!(!page.is_empty());
	assert!(
		page.iter()
			.all(|row| row["run"] == json!(fixture.run) && row["remote_grant"].is_null()),
		"{page:#?}"
	);
}

/// Adds what the 30 second loop covers for this Run: a command not yet dispatched
/// and a serving Live preview.
async fn running_targets(fixture: &Fixture) -> (Uuid, Uuid) {
	let driver = fixture.driver();
	let area = Uuid::new_v4();
	sqlx::query("INSERT INTO core_areas (id,tenant,home_node,agent_id,owner,workspace_id,thread_id,state) VALUES ($1,'acme',$2,'fixture','reader',$3,$4,'running')")
		.bind(area).bind(DEPLOYMENT).bind(fixture.workspace).bind(Uuid::new_v4())
		.execute(driver).await.unwrap();
	sqlx::query("INSERT INTO core_manifest_revisions (tenant,area_id,revision,manifest) VALUES ('acme',$1,1,'[]')")
		.bind(area)
		.execute(driver)
		.await
		.unwrap();
	let operation = Uuid::new_v4();
	sqlx::query("INSERT INTO core_operations (id,area_id,run_id,tenant,principal,credential_id,subjects,request_key,digest,kind,state,epoch,generation,revision,policy_revision,input,result) VALUES ($1,$2,$3,'acme','reader',$4,$5,$1::text,'digest','shell','prepared',1,1,1,1,'{}','{}')")
		.bind(operation).bind(area).bind(fixture.run).bind(fixture.identity.credential_id).bind(json!(["reader","helper"]))
		.execute(driver).await.unwrap();
	let preview = Uuid::new_v4();
	let now = chrono::Utc::now();
	sqlx::query("INSERT INTO core_live_previews (id,tenant,run_id,area_id,revision,port,state,idle_seconds,absolute_seconds,started_at,last_request_at,expires_at) VALUES ($1,'acme',$2,$3,1,3000,'running',900,7200,$4,$4,$5)")
		.bind(preview).bind(fixture.run).bind(area).bind(now).bind(now + chrono::Duration::hours(2))
		.execute(driver).await.unwrap();
	(operation, preview)
}

async fn operation_state(fixture: &Fixture, id: Uuid) -> (String, Value) {
	sqlx::query_as("SELECT state, result FROM core_operations WHERE id=$1")
		.bind(id)
		.fetch_one(fixture.driver())
		.await
		.unwrap()
}

async fn preview_stop(fixture: &Fixture, id: Uuid) -> (String, Option<String>) {
	sqlx::query_as("SELECT state, stop_reason FROM core_live_previews WHERE id=$1")
		.bind(id)
		.fetch_one(fixture.driver())
		.await
		.unwrap()
}

async fn remote_preview(fixture: &Fixture, preview: Uuid, with_session: bool) {
	sqlx::query("DELETE FROM authorization_execution WHERE run_id = $1")
		.bind(fixture.run)
		.execute(fixture.driver())
		.await
		.unwrap();
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	// Keep the read journal local while exercising the receiver admission path.
	// The preview has no local grant, as on a Remote Grant Run.
	sqlx::query("UPDATE runs SET home_node = $1 WHERE id = $2")
		.bind(format!("{DEPLOYMENT}/{node}"))
		.bind(fixture.run)
		.execute(fixture.driver())
		.await
		.unwrap();
	sqlx::query("INSERT INTO authorization_remote_admissions (id,node,source_node,grant_id,task_id,tenant,credential_id,subject_chain,description) VALUES ($1,$2,$3,$4,$5,'acme',$6,$7,'{}')")
		.bind(fixture.run).bind(node).bind(DEPLOYMENT).bind(Uuid::new_v4()).bind(fixture.task)
		.bind(fixture.identity.credential_id).bind(vec!["reader", "helper"])
		.execute(fixture.driver()).await.unwrap();
	if with_session {
		sqlx::query("INSERT INTO egress_sessions (id,tenant,run_id,live_preview_id,principal,credential_id,expires_at,max_sent_bytes,max_received_bytes,max_requests) VALUES ($1,'acme',$2,$1,'reader',$3,clock_timestamp()+interval '1 hour',100,100,10)")
			.bind(preview).bind(fixture.run).bind(fixture.identity.credential_id)
			.execute(fixture.driver()).await.unwrap();
	}
}

#[rstest]
#[case::with_session(true)]
#[case::without_session(false)]
#[tokio::test]
async fn remote_preview_recheck_without_receiver_workspace(
	#[future] database: DatabaseFixture,
	#[case] with_session: bool,
) {
	use aidash_application::ports::capabilities::read_recheck::{
		Outcome, ReadRecheckRepository, Target,
	};
	let fixture = fixture(database.await, 1).await;
	let (_, preview) = running_targets(&fixture).await;
	remote_preview(&fixture, preview, with_session).await;
	let remote_workspace = Uuid::new_v4();
	sqlx::query("UPDATE runs SET workspace_id = $1 WHERE id = $2")
		.bind(remote_workspace)
		.bind(fixture.run)
		.execute(fixture.driver())
		.await
		.unwrap();
	let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = $1)")
		.bind(remote_workspace)
		.fetch_one(fixture.driver())
		.await
		.unwrap();
	assert!(!exists);
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	let target = Target::LivePreview(preview);
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Allowed);
	fixture
		.set_policy(1, Some(("helper", "message.read")))
		.await;
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Refused);
}

#[rstest]
#[tokio::test]
async fn remote_preview_recheck_stops_when_a_chain_member_loses_read(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::ports::capabilities::read_recheck::{
		Outcome, ReadRecheckRepository, Target,
	};
	let fixture = fixture(database.await, 1).await;
	let (_, preview) = running_targets(&fixture).await;
	remote_preview(&fixture, preview, true).await;
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	let target = Target::LivePreview(preview);
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Allowed);
	fixture
		.set_policy(1, Some(("helper", "message.read")))
		.await;
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Refused);
	repository.stop_refused(target).await.unwrap();
	assert_eq!(
		preview_stop(&fixture, preview).await,
		("stopped".into(), Some("run_authority_revoked".into()))
	);
}

#[rstest]
#[tokio::test]
async fn remote_preview_recheck_still_checks_a_viewer_outside_the_chain(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::ports::capabilities::read_recheck::{
		Outcome, ReadRecheckRepository, Target,
	};
	let fixture = fixture(database.await, 1).await;
	let (_, preview) = running_targets(&fixture).await;
	remote_preview(&fixture, preview, true).await;
	let credential = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_credentials (id,tenant,subject,token_hash,created_at,expires_at,issued_by) VALUES ($1,'acme','viewer',$2,clock_timestamp(),clock_timestamp()+interval '1 day','fixture')")
		.bind(credential).bind(vec![2u8; 32]).execute(fixture.driver()).await.unwrap();
	sqlx::query(
		"UPDATE egress_sessions SET principal = 'viewer', credential_id = $1 WHERE id = $2",
	)
	.bind(credential)
	.bind(preview)
	.execute(fixture.driver())
	.await
	.unwrap();
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	let target = Target::LivePreview(preview);
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Allowed);
	fixture
		.set_policy(1, Some(("viewer", "message.read")))
		.await;
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Refused);
}

#[rstest]
#[tokio::test]
async fn remote_preview_recheck_without_session_is_unconfirmed_and_stops(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::{
		capabilities::read_recheck::{self, PERIOD, Schedule, UNCONFIRMED_LIMIT},
		ports::capabilities::read_recheck::{Outcome, ReadRecheckRepository, Target},
	};
	let mut fixture = fixture(database.await, 1).await;
	std::sync::Arc::make_mut(&mut fixture.store.capabilities.0).egress = Some(
		crate::apps::execution::capabilities::serializers::core::EgressSettings {
			endpoint: "http://10.96.0.90:8950".into(),
			namespace: None,
			pod_selector: Default::default(),
			listen: "127.0.0.1:0".parse().unwrap(),
			proxy_global_tunnels: 1024,
			proxy_tenant_tunnels: 32,
			limits: Default::default(),
		},
	);
	let (_, preview) = running_targets(&fixture).await;
	remote_preview(&fixture, preview, false).await;
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	assert_eq!(
		repository
			.check(Target::LivePreview(preview))
			.await
			.unwrap(),
		Outcome::Unconfirmed
	);
	let mut schedule = Schedule::default();
	let now = std::time::Instant::now();
	for count in 0..UNCONFIRMED_LIMIT {
		read_recheck::pass(&repository, &mut schedule, now + PERIOD * count)
			.await
			.unwrap();
	}
	assert_eq!(
		preview_stop(&fixture, preview).await,
		("stopped".into(), Some("node_unreachable".into()))
	);
}

#[rstest]
#[tokio::test]
async fn remote_preview_recheck_without_admission_is_unconfirmed(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::ports::capabilities::read_recheck::{
		Outcome, ReadRecheckRepository, Target,
	};
	let fixture = fixture(database.await, 1).await;
	let (_, preview) = running_targets(&fixture).await;
	remote_preview(&fixture, preview, true).await;
	sqlx::query("DELETE FROM authorization_remote_admissions WHERE id = $1")
		.bind(fixture.run)
		.execute(fixture.driver())
		.await
		.unwrap();
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	assert_eq!(
		repository
			.check(Target::LivePreview(preview))
			.await
			.unwrap(),
		Outcome::Unconfirmed
	);
}

#[rstest]
#[tokio::test]
async fn remote_preview_recheck_with_an_empty_chain_is_unconfirmed(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::ports::capabilities::read_recheck::{
		Outcome, ReadRecheckRepository, Target,
	};
	let fixture = fixture(database.await, 1).await;
	let (_, preview) = running_targets(&fixture).await;
	remote_preview(&fixture, preview, true).await;
	sqlx::query("UPDATE authorization_remote_admissions SET subject_chain = '{}' WHERE id = $1")
		.bind(fixture.run)
		.execute(fixture.driver())
		.await
		.unwrap();
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	assert_eq!(
		repository
			.check(Target::LivePreview(preview))
			.await
			.unwrap(),
		Outcome::Unconfirmed
	);
}

#[rstest]
#[case::the_person("reader")]
#[case::only_someone_else_on_the_chain("helper")]
#[tokio::test]
async fn the_loop_stops_what_is_running_when_a_read_is_revoked(
	#[future] database: DatabaseFixture,
	#[case] subject: &str,
) {
	use aidash_application::ports::capabilities::read_recheck::{
		Outcome, ReadRecheckRepository, Target,
	};
	// Arrange: the Run has finished, and its Live preview keeps serving.
	let fixture = fixture(database.await, 3).await;
	let (operation, preview) = running_targets(&fixture).await;
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	let targets = repository.targets().await.unwrap();
	assert!(
		targets.contains(&Target::Operation(operation)),
		"{targets:?}"
	);
	assert!(
		targets.contains(&Target::LivePreview(preview)),
		"{targets:?}"
	);
	for target in [Target::Operation(operation), Target::LivePreview(preview)] {
		assert_eq!(repository.check(target).await.unwrap(), Outcome::Allowed);
	}

	// Act
	fixture.set_policy(1, Some((subject, "message.read"))).await;
	let targets = [Target::Operation(operation), Target::LivePreview(preview)];
	for target in targets {
		assert_eq!(repository.check(target).await.unwrap(), Outcome::Refused);
		repository.stop_refused(target).await.unwrap();
	}

	// Assert
	let (state, result) = operation_state(&fixture, operation).await;
	assert_eq!(state, "withdrawn");
	assert_eq!(result["error"], "AUTHORITY_WITHDRAWN");
	assert_eq!(
		preview_stop(&fixture, preview).await,
		(
			"stopped".to_owned(),
			Some("run_authority_revoked".to_owned())
		)
	);
	let targets = repository.targets().await.unwrap();
	assert!(targets.is_empty(), "{targets:?}");
}

#[rstest]
#[tokio::test]
async fn a_live_preview_that_cannot_be_confirmed_stops_without_blaming_authority(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::ports::capabilities::read_recheck::ReadRecheckRepository;
	// Arrange
	let fixture = fixture(database.await, 1).await;
	let (_, preview) = running_targets(&fixture).await;
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};

	// Act
	repository.stop_unconfirmed(preview).await.unwrap();

	// Assert
	assert_eq!(
		preview_stop(&fixture, preview).await,
		("stopped".to_owned(), Some("node_unreachable".to_owned()))
	);
}

#[rstest]
#[tokio::test]
async fn a_recheck_that_only_repeats_adds_no_rows_for_a_running_command(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::ports::capabilities::read_recheck::{
		Outcome, ReadRecheckRepository, Target,
	};
	// Arrange
	let fixture = fixture(database.await, 3).await;
	let (operation, _) = running_targets(&fixture).await;
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	let target = Target::Operation(operation);
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Allowed);
	let written = fixture.rows("true").await;
	assert_eq!(
		fixture.rows("NOT coalesced").await,
		0,
		"every row of a Read Recheck is coalesced"
	);

	// Act
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Allowed);

	// Assert
	assert_eq!(fixture.rows("true").await, written);
}

/// The sweep loads a running command on every pass, about every 300 ms. Loading is
/// its Continuation Check, so a pass that only repeats it adds no row.
#[rstest]
#[tokio::test]
async fn a_sweep_pass_that_only_repeats_adds_no_rows_for_a_running_command(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::ports::capabilities::reconciliation::OperationReconciliationRepository;
	// Arrange: the command's Run is bound to its Working Area, as a started Run is.
	let fixture = fixture(database.await, 1).await;
	// The sweep also rechecks the Run's Agent under its current Catalog Approval.
	let model = json!({
		"id":"model", "version":"1.0.0", "kind":"model",
		"name":{"en":"Model"}, "description":{"en":"Fixture"},
		"config":{"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:9/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}
	});
	sqlx::query("INSERT INTO registry (tenant,id,version,kind,metadata) VALUES ('acme','model','1.0.0','model',$1)")
		.bind(model)
		.execute(fixture.driver())
		.await
		.unwrap();
	let entry: crate::registry::Entry = serde_json::from_value(json!({
		"id":"fixture", "version":"1.0.0", "kind":"agent",
		"name":{"en":"Fixture"}, "description":{"en":"Fixture"},
		"config":{"schema_version":1,"model":{"id":"model","version":"1.0.0"},"instructions":"Work","bindings":[],"remove_default":[]}
	})).unwrap();
	sqlx::query(
		"INSERT INTO registry (tenant,id,version,kind,metadata) VALUES ('acme',$1,$2,'agent',$3)",
	)
	.bind(&entry.id)
	.bind(&entry.version)
	.bind(serde_json::to_value(&entry).unwrap())
	.execute(fixture.driver())
	.await
	.unwrap();
	Authorization {
		pool: fixture.store.pool.clone(),
	}
	.set_catalog(
		"acme",
		&aidash_domain::nodes::NodeName::derived("acme"),
		&crate::registry::EntityRef {
			id: entry.id,
			version: entry.version,
		},
		0,
		true,
		"fixture",
	)
	.await
	.unwrap();
	let (operation, _) = running_targets(&fixture).await;
	sqlx::query("INSERT INTO core_runs (run_id,tenant,area_id,sequence,initialized,generation) SELECT $1,'acme',id,1,true,generation FROM core_areas WHERE id=(SELECT area_id FROM core_operations WHERE id=$2)")
		.bind(fixture.run)
		.bind(operation)
		.execute(fixture.driver())
		.await
		.unwrap();
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	let pass = async || {
		let snapshot = repository.snapshot(operation).await.unwrap();
		let mut scope = repository.begin(&snapshot).await.unwrap();
		scope.load(operation, snapshot.run_id).await.unwrap();
		scope.finish(Ok(())).await.unwrap();
	};
	let before = fixture.rows("true").await;
	pass().await;
	let written = fixture.rows("true").await;
	assert!(written > before, "the first pass records what it allowed");

	// Act
	pass().await;

	// Assert
	assert_eq!(fixture.rows("true").await, written);
}

#[rstest]
#[tokio::test]
async fn a_run_reached_two_ways_in_one_walk_is_checked_once(#[future] database: DatabaseFixture) {
	// Arrange: the Run read `shared` directly and through `between`.
	let fixture = fixture(database.await, 1).await;
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	let [shared, between] = [Uuid::new_v4(), Uuid::new_v4()];
	for run in [shared, between] {
		let task = Uuid::new_v4();
		sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,'acme',$2,'Fixture','Fixture','COMPLETED',1,'reader')").bind(task).bind(fixture.workspace).execute(fixture.driver()).await.unwrap();
		sqlx::query("INSERT INTO runs (id,tenant,task_id,workspace_id,node,home_node,agent_id,agent_version,phase,pending,revision) VALUES ($1,'acme',$2,$3,$4,$5,'fixture','1.0.0','COMPLETED',$6,1)").bind(run).bind(task).bind(fixture.workspace).bind(&node).bind(DEPLOYMENT).bind(pending.clone()).execute(fixture.driver()).await.unwrap();
	}
	let mut tx = fixture
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	// Every Run executes under a grant (ADR 0043).
	for run in [shared, between] {
		let task: Uuid = sqlx::query_scalar("SELECT task_id FROM runs WHERE id=$1")
			.bind(run)
			.fetch_one(fixture.driver())
			.await
			.unwrap();
		crate::apps::identity::repositories::admission::persist_grant_in(
			&mut *tx,
			&ExecutionGrant {
				run_id: run,
				task_id: task,
				workspace_id: fixture.workspace,
				tenant: "acme".into(),
				credential_id: fixture.identity.credential_id,
				root_subject: "reader".into(),
				subject_chain: vec!["reader".into(), "helper".into()],
			},
		)
		.await
		.unwrap();
	}
	for (reader, read) in [
		(fixture.run, shared),
		(fixture.run, between),
		(between, shared),
	] {
		crate::apps::identity::models::AuthorizationRunRead::record_sources(
			&mut *tx,
			reader,
			fixture.workspace,
			&[("run".to_owned(), read)],
		)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	let mut access = fixture.access().await;

	// Act
	let (result, statements) = recorded(access.recheck_run_reads(fixture.run)).await;

	// Assert
	result.unwrap();
	let human_checks = statements
		.iter()
		.filter(|statement| {
			statement.contains("human_requests") && statement.contains(&shared.to_string())
		})
		.count();
	assert_eq!(human_checks, 1, "{statements:#?}");
	let walks = statements
		.iter()
		.filter(|statement| {
			statement.contains("authorization_run_reads") && statement.contains(&shared.to_string())
		})
		.count();
	assert_eq!(walks, 1, "{statements:#?}");
	access.finish(Ok(())).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn a_failed_walk_is_not_remembered_as_a_pass(#[future] database: DatabaseFixture) {
	// Arrange: a list asks about the same Run twice, under a policy that denies
	// the Message it read. The second question must not pass on the first's visit.
	let fixture = fixture(database.await, 1).await;
	fixture
		.set_policy(1, Some(("reader", "message.read")))
		.await;
	let mut access = fixture.access().await;

	// Act
	let first = access.run_reads_visible(fixture.run).await.unwrap();
	access.read_verdicts.clear();
	let second = access.run_reads_visible(fixture.run).await.unwrap();

	// Assert
	assert!(!first);
	assert!(!second);
	access.finish(Ok(())).await.unwrap();
}

#[rstest]
#[tokio::test]
async fn a_passed_walk_is_not_reused_under_another_authority(#[future] database: DatabaseFixture) {
	// Arrange: only `helper` has lost the read.
	let fixture = fixture(database.await, 1).await;
	fixture
		.set_policy(1, Some(("helper", "message.read")))
		.await;
	let mut access = fixture.access().await;

	// Act: the same Access, first for `reader` alone and then for the whole chain.
	let alone = access.run_reads_visible(fixture.run).await.unwrap();
	access.subjects = vec!["reader".into(), "helper".into()];
	let chain = access.run_reads_visible(fixture.run).await.unwrap();

	// Assert
	assert!(alone);
	assert!(!chain, "the chain's authority is asked afresh");
	access.finish(Ok(())).await.unwrap();
}

/// One authority serves several calls, as it does for a Tool Batch. The Task a
/// call asks for is refused to the person, and an earlier call on the same
/// authority found a Node silent: the refusal reaches the caller as a refusal,
/// not as that Node's silence, which would be retried as undelivered (#751).
#[rstest]
#[tokio::test]
async fn a_refusal_is_not_blamed_on_a_node_an_earlier_call_found_silent(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let fixture = fixture(database.await, 1).await;
	fixture.set_policy(1, Some(("reader", "task.read"))).await;
	let mut access = fixture.access().await;
	access.mark_peer_unavailable("aidash://other/sibling");

	// Act
	let refused = access.task_read(fixture.task).await;
	access.finish(Ok(())).await.unwrap();

	// Assert
	assert!(matches!(refused, Err(Error::Forbidden)), "{refused:?}");
}

/// An authority asks other Nodes inside the distribution lock its transaction
/// holds, so a Node of this Deployment reached in process requests none of its
/// own (#751). An authority under a lease holds what the lease holds, and none
/// is claimed once a savepoint rollback may have released it.
#[rstest]
#[tokio::test]
async fn an_authority_reaches_other_nodes_inside_the_distribution_lock_it_holds(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let fixture = fixture(database.await, 1).await;
	let mut outer = fixture.access().await;
	assert!(
		!outer.reach().1.held(),
		"nothing is held before a lock is taken"
	);

	// Act and assert: a shared lock taken by a Catalog admission is held.
	crate::marketplace::lock_catalog(&mut outer.tx, false)
		.await
		.unwrap();
	assert!(outer.reach().1.held());
	let reach = outer.reach();
	let inner = Access::under_lease(&outer).await.unwrap();
	assert!(
		inner.reach().1.held(),
		"an authority under the lease holds the lease's lock"
	);
	inner.finish(Ok(())).await.unwrap();

	// A savepoint rollback may release it, and the lock is claimed again when it is taken.
	outer.tx.savepoint("probe").await.unwrap();
	outer.tx.rollback_to_savepoint("probe").await.unwrap();
	assert!(!outer.reach().1.held());
	assert!(
		!reach.1.held(),
		"a hold read earlier follows the transaction, as the lock does"
	);
	crate::marketplace::lock_catalog(&mut outer.tx, false)
		.await
		.unwrap();
	assert!(outer.reach().1.held());
	outer.finish(Ok(())).await.unwrap();
}

/// A Run that has finished, with the grant every Run executes under (ADR 0043).
async fn finished_run(fixture: &Fixture) -> Uuid {
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	let [run, task] = [Uuid::new_v4(), Uuid::new_v4()];
	sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,'acme',$2,'Fixture','Fixture','COMPLETED',1,'reader')").bind(task).bind(fixture.workspace).execute(fixture.driver()).await.unwrap();
	sqlx::query("INSERT INTO runs (id,tenant,task_id,workspace_id,node,home_node,agent_id,agent_version,phase,pending,revision) VALUES ($1,'acme',$2,$3,$4,$5,'fixture','1.0.0','COMPLETED',$6,1)").bind(run).bind(task).bind(fixture.workspace).bind(&node).bind(DEPLOYMENT).bind(pending).execute(fixture.driver()).await.unwrap();
	let mut tx = fixture
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	crate::apps::identity::repositories::admission::persist_grant_in(
		&mut *tx,
		&ExecutionGrant {
			run_id: run,
			task_id: task,
			workspace_id: fixture.workspace,
			tenant: "acme".into(),
			credential_id: fixture.identity.credential_id,
			root_subject: "reader".into(),
			subject_chain: vec!["reader".into(), "helper".into()],
		},
	)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	run
}

/// Everything is allowed, except `subject` reading the one `resource`.
fn policy_hiding(subject: &str, action: &str, resource: Uuid) -> Value {
	let mut bundle = policy(None);
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"hidden", "effect":"deny", "subjects":{"ids":[subject]},
		"actions":[action], "resources":{"kinds":["*"],"ids":[resource.to_string()]}
	}));
	bundle
}

/// Whether the viewer's own walk over the Run `fixture.run` passes.
async fn viewer_passes(access: &mut Access, fixture: &Fixture) -> bool {
	access.run_reads_visible(fixture.run).await.unwrap()
}

/// What a Remote Grant's producer sees of `message`, a source the grant discloses.
async fn disclosure_passes(
	access: &mut Access,
	grant: &aidash_domain::federation::execution::home::Grant,
	message: Uuid,
	workspace: Uuid,
) -> bool {
	let row = crate::apps::workspaces::models::Message::read_in(
		access.tx.as_mut(),
		message,
		workspace,
		true,
	)
	.await
	.unwrap()
	.unwrap();
	let producer =
		crate::apps::identity::repositories::remote_grants::reads::Producer::lend(access, grant)
			.await
			.unwrap();
	producer.access.message_visible(&row).await.unwrap()
}

/// Run X read a Task nobody else read, and produced the Message the viewer's Run
/// read as well as the one a Remote Grant discloses. The viewer's walk and the
/// grant's producer both reach X, one under the viewer's chain and one under the
/// grant's, so each must read X's reads for itself, whichever comes first.
#[rstest]
#[case::the_viewer_first_and_the_grants_root_lost_the_read("helper", "reader", true)]
#[case::the_grant_first_and_the_viewer_lost_the_read("helper", "helper", false)]
#[tokio::test]
async fn a_grants_producer_reads_a_run_for_its_own_chain_whichever_walk_reached_it_first(
	#[future] database: DatabaseFixture,
	#[case] viewer: &str,
	#[case] lost: &str,
	#[case] viewer_first: bool,
) {
	// Arrange
	let fixture = fixture(database.await, 1).await;
	let x = finished_run(&fixture).await;
	let [x_only, disclosed] = [Uuid::new_v4(), Uuid::new_v4()];
	sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,'acme',$2,'Only X','Fixture','COMPLETED',1,'reader')").bind(x_only).bind(fixture.workspace).execute(fixture.driver()).await.unwrap();
	sqlx::query("INSERT INTO messages (id,tenant,workspace_id,sender,content) VALUES ($1,'acme',$2,'reader','Disclosed')").bind(disclosed).bind(fixture.workspace).execute(fixture.driver()).await.unwrap();
	let read: Uuid = sqlx::query_scalar(
		"SELECT resource_id FROM authorization_run_reads WHERE run_id=$1 AND resource_kind='message'",
	)
	.bind(fixture.run)
	.fetch_one(fixture.driver())
	.await
	.unwrap();
	let mut tx = fixture
		.store
		.pool
		.begin(&TenancyScope::tenant("acme").unwrap())
		.await
		.unwrap();
	crate::apps::identity::models::AuthorizationRunRead::record_sources(
		&mut *tx,
		x,
		fixture.workspace,
		&[("task".to_owned(), x_only)],
	)
	.await
	.unwrap();
	for message in [read, disclosed] {
		crate::apps::identity::models::AuthorizationRunOutput::record(
			&mut *tx,
			x,
			fixture.workspace,
			"message",
			message,
		)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	Authorization {
		pool: fixture.store.pool.clone(),
	}
	.replace(
		"acme",
		1,
		serde_json::from_value(policy_hiding(lost, "task.read", x_only)).unwrap(),
		"deployment_administrator",
	)
	.await
	.unwrap();
	let grant = aidash_domain::federation::execution::home::Grant {
		id: Uuid::new_v4(),
		task_id: fixture.task,
		task_revision: 1,
		workspace_id: fixture.workspace,
		node_id: "aidash://peer".into(),
		tenant: "acme".into(),
		credential_id: fixture.identity.credential_id,
		root_subject: "reader".into(),
		subject_chain: vec!["reader".into()],
		inspection: json!({}),
		expires_at: chrono::Utc::now() + chrono::Duration::days(1),
		revoked: false,
		semantic: json!({}),
	};
	let mut access = fixture.access().await;
	access.subjects = vec![viewer.to_owned()];
	// The viewer's walk is under way when it reaches the grant's output, as in
	// `grant_visible`.
	let outermost = access.reads_idle();
	let _visit = access.authority_read_visit("run", fixture.run);
	let walk = access.begin_read_walk(DecisionOwner::Run(fixture.run), outermost);

	// Act
	let (viewer_sees, grant_sees) = if viewer_first {
		let viewer_sees = viewer_passes(&mut access, &fixture).await;
		let grant_sees = disclosure_passes(&mut access, &grant, disclosed, fixture.workspace).await;
		(viewer_sees, grant_sees)
	} else {
		let grant_sees = disclosure_passes(&mut access, &grant, disclosed, fixture.workspace).await;
		let viewer_sees = viewer_passes(&mut access, &fixture).await;
		(viewer_sees, grant_sees)
	};

	// Assert: only the chain that lost the read is refused.
	assert_eq!(
		(viewer_sees, grant_sees),
		(lost != viewer, lost != "reader"),
		"viewer, grant"
	);
	access.end_read_walk(walk, Ok(false)).await.unwrap();
	access.finish(Ok(())).await.unwrap();
}

/// Egress refuses a stream from the operation row. When the Runner cannot be
/// asked to stop a command, that row must still say its authority is gone.
#[rstest]
#[tokio::test]
async fn a_command_is_marked_withdrawn_even_when_the_runner_cannot_be_asked(
	#[future] database: DatabaseFixture,
) {
	use aidash_application::ports::capabilities::read_recheck::{
		Outcome, ReadRecheckRepository, Target,
	};
	// Arrange: the Runner holds the command, and this Store has no Runner to ask.
	let fixture = fixture(database.await, 3).await;
	let (operation, _) = running_targets(&fixture).await;
	sqlx::query(
		"UPDATE core_operations SET state='running', runner_instance='runner-1' WHERE id=$1",
	)
	.bind(operation)
	.execute(fixture.driver())
	.await
	.unwrap();
	let repository = crate::apps::execution::repositories::operations::reconciliation::Repository {
		store: &fixture.store,
	};
	let target = Target::Operation(operation);
	fixture
		.set_policy(1, Some(("reader", "message.read")))
		.await;
	assert_eq!(repository.check(target).await.unwrap(), Outcome::Refused);

	// Act
	let stopped = repository.stop_refused(target).await;

	// Assert
	assert!(stopped.is_err(), "no Runner could stop it");
	let (state, result) = operation_state(&fixture, operation).await;
	assert_eq!(state, "cancelling");
	assert_eq!(result["error"], "AUTHORITY_WITHDRAWN");
	let targets = repository.targets().await.unwrap();
	assert!(
		!targets.contains(&target),
		"the loop is done with it: the sweep goes on asking the Runner, {targets:?}"
	);
}

/// A Continuation Check or Read Recheck repeats the same question, so its allows
/// add no row. What the Access is asked after it is not part of it.
#[rstest]
#[tokio::test]
async fn what_an_access_is_asked_after_a_check_is_recorded_as_it_is_today(
	#[future] database: DatabaseFixture,
) {
	// Arrange
	let fixture = fixture(database.await, 1).await;
	let mut access = fixture.access().await;

	// Act: the check ends on every path, a refusal included.
	let unknown = access.run_for_continuation(Uuid::new_v4()).await;
	assert!(matches!(unknown, Err(Error::Forbidden)), "{unknown:?}");
	assert!(access.recheck_owner.is_none());
	access.run_for_continuation(fixture.run).await.unwrap();
	assert!(access.recheck_owner.is_none());
	access.recheck_run_reads(fixture.run).await.unwrap();
	assert!(access.recheck_owner.is_none());
	let later = access.resource("note", "later", json!({}));
	assert!(access.decide(&later, "note.read").await.unwrap());
	access.finish(Ok(())).await.unwrap();

	// Assert
	assert_eq!(
		fixture.rows("action = 'note.read' AND NOT coalesced").await,
		1
	);
	assert_eq!(fixture.rows("action = 'note.read' AND coalesced").await, 0);
}

/// The fence a Subnode puts on writes closes between two identical checks, so a
/// repeat must not be answered from what the first one allowed.
#[rstest]
#[tokio::test]
async fn a_write_a_subnode_fences_is_checked_afresh_inside_a_check(
	#[future] database: DatabaseFixture,
) {
	// Arrange: the Run's own Workspace is an active Subnode.
	let fixture = fixture(database.await, 1).await;
	sqlx::query(
		"INSERT INTO subnodes (id, tenant, node, creator_run_id, creator_task_id, \
		  creator_agent_id, creator_agent_version, leader_agent_id, leader_agent_version, \
		  allowance, workspace_id, title, state, created_at, creation_key, creation_fingerprint) \
		 SELECT $1, tenant, node, id, task_id, agent_id, agent_version, agent_id, agent_version, \
		  $2, workspace_id, 'Fenced', 'active', now(), 'fenced', 'fenced' \
		 FROM runs WHERE id = $3",
	)
	.bind(Uuid::new_v4())
	.bind(json!({"tools":[],"secrets":[]}))
	.bind(fixture.run)
	.execute(fixture.driver())
	.await
	.unwrap();
	let mut access = fixture.access().await;
	let workspace = access.resource("workspace", fixture.workspace, json!({}));
	let mut check = access.rechecking(fixture.run);
	assert!(check.decide(&workspace, "workspace.update").await.unwrap());

	// Act: the Subnode ends in this transaction, which holds its share lock.
	crate::database::native::query(&format!(
		"UPDATE subnodes SET state='dissolved', ended_at=now(), ended_reason='dissolved' \
		 WHERE workspace_id='{}'",
		fixture.workspace
	))
	.execute(&mut **check.tx)
	.await
	.unwrap();
	let repeat = check.decide(&workspace, "workspace.update").await;

	// Assert
	assert!(matches!(repeat, Err(Error::Forbidden)), "{repeat:?}");
}
