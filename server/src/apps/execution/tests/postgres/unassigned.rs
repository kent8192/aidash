//! Unassigned Tasks against real rows (#772): what the child summary says,
//! how a completion or a person abandons them, and what a finishing Run does.
use super::*;
use aidash_server::domain::TaskStatus;

async fn workspace_with_parent(f: &Federation, agent: &Entry) -> (Uuid, Task) {
	let workspace = f
		.store
		.create_workspace(&ACME, "alice", "Unassigned", "Subtasks nobody runs")
		.await
		.unwrap();
	let parent = running_task(f, agent, workspace.id, None).await;
	(workspace.id, parent)
}

async fn run_of(store: &Store, task: Uuid) -> Uuid {
	sqlx::query_scalar("SELECT id FROM runs WHERE task_id = $1")
		.bind(task)
		.fetch_one(store.pool.fixture_driver())
		.await
		.unwrap()
}

/// A child a person made: no origin row.
async fn person_child(f: &Federation, workspace: Uuid, parent: Uuid, title: &str) -> Task {
	let mut input = new_task();
	input.title = title.into();
	input.parent_id = Some(parent);
	f.store
		.create_task(&ACME, workspace, &input, "alice", None)
		.await
		.unwrap()
}

/// A child a Run of the parent made, the way `task_create` records it.
async fn run_child(f: &Federation, workspace: Uuid, parent: &Task, title: &str) -> Task {
	let child = person_child(f, workspace, parent.id, title).await;
	let run = run_of(&f.store, parent.id).await;
	sqlx::query("INSERT INTO authorization_task_origins (task_id, tenant, source_run_id, root_subject, subject_chain) VALUES ($1, 'acme', $2, 'alice', ARRAY['alice','fixture-agent'])")
		.bind(child.id)
		.bind(run)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	child
}

async fn bind_remote(f: &Federation, task: &Task, activated: bool, stale_revision: bool) {
	let revision = if stale_revision {
		task.revision - 1
	} else {
		task.revision
	};
	sqlx::query("INSERT INTO authorization_remote_execution (grant_id, tenant, admission_id, task_id, task_revision, initial_task, human_requests, activated_at) VALUES ($1, 'acme', $2, $3, $4, '{}'::jsonb, '[]'::jsonb, CASE WHEN $5 THEN CURRENT_TIMESTAMP END)")
		.bind(Uuid::new_v4())
		.bind(Uuid::new_v4())
		.bind(task.id)
		.bind(revision)
		.bind(activated)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
}

/// Put a Task in CLAIMED without the dependency check `claim` applies. Claiming
/// refuses a Task whose dependencies are unfinished, so this state is only
/// reached by hand; the summary must still read it as handed over.
async fn force_claimed(f: &Federation, task: &Task) {
	sqlx::query("UPDATE tasks SET status = 'CLAIMED', owner = 'node/agent@1', revision = revision + 1 WHERE id = $1")
		.bind(task.id)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
}

fn artifact() -> ArtifactInput {
	ArtifactInput {
		kind: "text".into(),
		name: "Result".into(),
		content: json!("Done."),
	}
}

fn owner_of(task: &Task) -> String {
	task.owner.clone().unwrap()
}

async fn complete(f: &Federation, parent: &Task, abandon: &[Uuid]) -> aidash_server::Result<Task> {
	f.store
		.complete_abandoning(
			&ACME,
			parent.id,
			&owner_of(parent),
			"parent:complete",
			&artifact(),
			abandon,
		)
		.await
}

fn flags(summary: &ChildTaskSummary, id: Uuid) -> Option<(bool, bool)> {
	summary
		.unassigned
		.iter()
		.find(|task| task.id == id)
		.map(|task| (task.abandonable, task.handed_over))
}

#[rstest::rstest]
#[tokio::test]
async fn the_summary_names_each_unassigned_child_from_real_rows(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	// A: made by a Run of the parent. B: made by a person.
	let a = run_child(&f, workspace, &parent, "A").await;
	let b = person_child(&f, workspace, parent.id, "B").await;
	// C: made by a Run, with an open child of its own.
	let c = run_child(&f, workspace, &parent, "C").await;
	let mut under_c = new_task();
	under_c.parent_id = Some(c.id);
	store
		.create_task(&ACME, workspace, &under_c, "alice", None)
		.await
		.unwrap();
	// D: handed over (claimed) but waiting on A.
	let mut waiting = new_task();
	waiting.title = "D".into();
	waiting.parent_id = Some(parent.id);
	waiting.dependencies = vec![a.id];
	let d = run_child_with(&f, workspace, &parent, waiting).await;
	force_claimed(&f, &d).await;
	// E: activated on another Node. F: activation refused. G: a Run that ended.
	let e = run_child(&f, workspace, &parent, "E").await;
	bind_remote(&f, &e, true, false).await;
	let g = run_child(&f, workspace, &parent, "G").await;
	bind_remote(&f, &g, true, true).await;
	let h = run_child(&f, workspace, &parent, "F").await;
	bind_remote(&f, &h, false, false).await;

	let summary = store
		.child_task_summary(&ACME, workspace, parent.id)
		.await
		.unwrap();
	assert_eq!(flags(&summary, a.id), Some((true, false)));
	assert_eq!(flags(&summary, b.id), Some((false, false)));
	assert_eq!(
		flags(&summary, c.id),
		Some((false, false)),
		"open child beneath"
	);
	assert_eq!(flags(&summary, d.id).map(|(_, handed)| handed), Some(true));
	assert_eq!(flags(&summary, e.id), None, "activated on another Node");
	assert_eq!(
		flags(&summary, g.id).map(|(abandonable, _)| abandonable),
		Some(true),
		"reopened after its Run ended"
	);
	assert_eq!(
		flags(&summary, h.id).map(|(abandonable, _)| abandonable),
		Some(true),
		"refused activation"
	);
	// The six top-level children plus C's own child are all open.
	assert_eq!(summary.open_tasks, 7);
	cleanup(store, &url, &schema).await;
}

async fn run_child_with(f: &Federation, workspace: Uuid, parent: &Task, input: NewTask) -> Task {
	let child = f
		.store
		.create_task(&ACME, workspace, &input, "alice", None)
		.await
		.unwrap();
	let run = run_of(&f.store, parent.id).await;
	sqlx::query("INSERT INTO authorization_task_origins (task_id, tenant, source_run_id, root_subject, subject_chain) VALUES ($1, 'acme', $2, 'alice', ARRAY['alice','fixture-agent'])")
		.bind(child.id)
		.bind(run)
		.execute(f.store.pool.fixture_driver())
		.await
		.unwrap();
	child
}

#[rstest::rstest]
#[tokio::test]
async fn completing_abandons_the_listed_tasks_in_the_same_transaction(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let child = run_child(&f, workspace, &parent, "Nobody's").await;

	let done = complete(&f, &parent, &[child.id]).await.unwrap();
	assert_eq!(done.status, TaskStatus::Completed);
	assert_eq!(
		store.task(&ACME, child.id).await.unwrap().status,
		TaskStatus::Abandoned
	);
	let events = store.events(&ACME, 0, Some(workspace), 1000).await.unwrap();
	let abandoned: Vec<_> = events
		.iter()
		.filter(|event| event.kind == "task.abandoned")
		.collect();
	assert_eq!(abandoned.len(), 1);
	assert_eq!(
		abandoned[0].data["reason"],
		aidash_domain::unassigned::ABANDON_REASON
	);
	assert_eq!(abandoned[0].data["previous_status"], "OPEN");
	assert_eq!(abandoned[0].data["actor"], owner_of(&parent));

	// A replayed completion abandons nothing more and succeeds.
	let again = complete(&f, &parent, &[child.id]).await.unwrap();
	assert_eq!(again.status, TaskStatus::Completed);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_task_handed_over_just_before_completion_is_kept_and_nothing_commits(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let kept = run_child(&f, workspace, &parent, "Handed over").await;
	let other = run_child(&f, workspace, &parent, "Still nobody's").await;
	store
		.claim(&ACME, kept.id, kept.revision, "node/agent@1", &agent)
		.await
		.unwrap();

	let error = complete(&f, &parent, &[kept.id, other.id])
		.await
		.unwrap_err();
	assert!(
		matches!(error, aidash_server::Error::Conflict(_)),
		"{error:?}"
	);
	assert_eq!(
		store.task(&ACME, kept.id).await.unwrap().status,
		TaskStatus::Claimed
	);
	// The other abandonment rolled back with the failed completion.
	assert_eq!(
		store.task(&ACME, other.id).await.unwrap().status,
		TaskStatus::Open
	);
	assert_eq!(
		store.task(&ACME, parent.id).await.unwrap().status,
		TaskStatus::Running
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_persons_task_and_a_task_with_children_are_never_abandoned_by_a_completion(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let people = person_child(&f, workspace, parent.id, "A person's").await;
	assert!(complete(&f, &parent, &[people.id]).await.is_err());
	assert_eq!(
		store.task(&ACME, people.id).await.unwrap().status,
		TaskStatus::Open
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn abandoning_without_completing_leaves_a_handed_over_dependent_alone(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let root = run_child(&f, workspace, &parent, "Root").await;
	let handed_over = run_child(&f, workspace, &parent, "Handed over").await;
	bind_remote(&f, &handed_over, true, false).await;
	let mut dependent = new_task();
	dependent.parent_id = Some(parent.id);
	dependent.dependencies = vec![root.id];
	let dependent = run_child_with(&f, workspace, &parent, dependent).await;
	force_claimed(&f, &dependent).await;

	let abandoned = store
		.abandon_unassigned(
			&ACME,
			parent.id,
			&owner_of(&parent),
			&[root.id, handed_over.id],
		)
		.await
		.unwrap();
	assert_eq!(serde_json::to_value(abandoned).unwrap(), json!([root.id]));
	assert_eq!(
		store.task(&ACME, handed_over.id).await.unwrap().status,
		TaskStatus::Open
	);
	assert_eq!(
		store.task(&ACME, root.id).await.unwrap().status,
		TaskStatus::Abandoned
	);
	assert_eq!(
		store.task(&ACME, dependent.id).await.unwrap().status,
		TaskStatus::Claimed
	);
	assert_eq!(
		store.task(&ACME, parent.id).await.unwrap().status,
		TaskStatus::Running
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_person_can_abandon_an_open_task_nobody_started_but_not_one_with_a_run(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let free = person_child(&f, workspace, parent.id, "Free").await;
	let busy = person_child(&f, workspace, parent.id, "Busy").await;
	bind_remote(&f, &busy, true, false).await;

	// A Run under way (here: another Node confirmed it started) is refused.
	let refused = store
		.abandon_task(&ACME, busy.id, busy.revision, "Not needed")
		.await;
	assert!(matches!(refused, Err(aidash_server::Error::Conflict(_))));
	assert_eq!(
		store.task(&ACME, busy.id).await.unwrap().status,
		TaskStatus::Open
	);
	// A reason is still required.
	assert!(
		store
			.abandon_task(&ACME, free.id, free.revision, " ")
			.await
			.is_err()
	);
	let done = store
		.abandon_task(&ACME, free.id, free.revision, "Not needed")
		.await
		.unwrap();
	assert_eq!(done.status, TaskStatus::Abandoned);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_failed_parent_can_be_abandoned_once_its_unassigned_child_is(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let child = person_child(&f, workspace, parent.id, "Nobody's").await;
	let failed = store
		.transition(
			&ACME,
			parent.id,
			parent.revision,
			&owner_of(&parent),
			TaskStatus::Failed,
		)
		.await
		.unwrap();
	assert!(
		store
			.abandon_task(&ACME, failed.id, failed.revision, "Give up")
			.await
			.is_err(),
		"the open child blocks it"
	);
	store
		.abandon_task(&ACME, child.id, child.revision, "Not needed")
		.await
		.unwrap();
	let parent = store
		.abandon_task(&ACME, failed.id, failed.revision, "Give up")
		.await
		.unwrap();
	assert_eq!(parent.status, TaskStatus::Abandoned);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_finishing_run_tells_the_model_once_then_abandons_and_completes(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let child = run_child(&f, workspace, &parent, "Nobody's").await;
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};

	final_response(&store, parent.id).await;
	harness.worker_once().await.unwrap();
	let run_id = run_of(&store, parent.id).await;
	assert_eq!(
		store.run(&ACME, run_id).await.unwrap().phase().as_str(),
		"THINKING",
		"the first final answer is held and the model is told"
	);
	assert_eq!(
		store.task(&ACME, child.id).await.unwrap().status,
		TaskStatus::Open
	);

	final_response(&store, parent.id).await;
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.task(&ACME, parent.id).await.unwrap().status,
		TaskStatus::Completed
	);
	assert_eq!(
		store.task(&ACME, child.id).await.unwrap().status,
		TaskStatus::Abandoned
	);
	let artifacts = store.snapshot(&ACME, workspace).await.unwrap().artifacts;
	assert_eq!(artifacts.len(), 1);
	assert!(artifacts[0].content.as_str().unwrap().contains("Nobody's"));
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_task_a_person_made_raises_a_request_and_can_then_be_abandoned(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let child = person_child(&f, workspace, parent.id, "A person's").await;
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	for _ in 0..2 {
		final_response(&store, parent.id).await;
		harness.worker_once().await.unwrap();
	}
	let run = store
		.run(&ACME, run_of(&store, parent.id).await)
		.await
		.unwrap();
	assert_eq!(run.phase().as_str(), "WAITING");
	let request = request_id(&run);
	assert_eq!(
		store.task(&ACME, child.id).await.unwrap().status,
		TaskStatus::Open
	);

	store
		.abandon_task(&ACME, child.id, child.revision, "Not needed")
		.await
		.unwrap();
	store
		.answer(&ACME, request, json!("Go on without it"))
		.await
		.unwrap();
	harness.worker_once().await.unwrap();
	final_response(&store, parent.id).await;
	harness.worker_once().await.unwrap();
	assert_eq!(
		store.task(&ACME, parent.id).await.unwrap().status,
		TaskStatus::Completed
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn claiming_refuses_a_task_whose_dependency_is_unfinished(
	#[from(store)] _store_fixture: StoreFuture,
	#[from(federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	// Why a handed-over child waiting on an Unassigned sibling is not reached
	// through delegation: the claim inside admission needs finished dependencies.
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let (workspace, parent) = workspace_with_parent(&f, &agent).await;
	let root = run_child(&f, workspace, &parent, "Root").await;
	let mut input = new_task();
	input.parent_id = Some(parent.id);
	input.dependencies = vec![root.id];
	let dependent = run_child_with(&f, workspace, &parent, input).await;
	let refused = store
		.claim(
			&ACME,
			dependent.id,
			dependent.revision,
			"node/agent@1",
			&agent,
		)
		.await;
	assert!(matches!(refused, Err(aidash_server::Error::Conflict(_))));
	// Left OPEN with no Run, it is an Unassigned Task itself.
	let summary = store
		.child_task_summary(&ACME, workspace, parent.id)
		.await
		.unwrap();
	assert_eq!(flags(&summary, dependent.id), Some((true, false)));
	cleanup(store, &url, &schema).await;
}
