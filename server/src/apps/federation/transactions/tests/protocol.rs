//! Distributed transaction behavior over production Reinhardt routes.
use crate::fixtures::{Node, Pair, WorkerProcess, pair};
use aidash_server::{
	Error,
	apps::{
		execution::models::{Event, Run, states::RunPhase},
		federation::remote::models::Delegation,
		federation::transactions::{
			models::{
				AtomicCoordinator, AtomicHistory, AtomicParticipant, coordinator_records,
				states::{AtomicCoordinatorDecision, AtomicHistoryRole, AtomicParticipantPhase},
			},
			services::decisions::CoordinatorTransition,
		},
		workspaces::models::Workspace,
	},
	generation::provision,
	harness::Harness,
	transactions::{Mutation, Status, coordinator, participant},
};
use chrono::{Duration, Utc};
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use reinhardt::test::fixtures::http_client;
use rstest::rstest;
use serde_json::{Value, json};
use uuid::Uuid;

async fn workspace(node: &Node, id: Uuid) -> Workspace {
	Workspace::objects()
		.filter(Workspace::field_id().eq(id))
		.get_with_db(&mut node.database.lease.handle())
		.await
		.unwrap()
}

async fn updates(node: &Node, id: Uuid) -> usize {
	Event::objects()
		.filter(Event::field_workspace_id().eq(Some(id)))
		.filter(Event::field_kind().eq("workspace.updated"))
		.all_with_db(&mut node.database.lease.handle())
		.await
		.unwrap()
		.len()
}

async fn run_record(node: &Node, id: Uuid) -> Run {
	Run::objects()
		.filter(Run::field_id().eq(id))
		.get_with_db(&mut node.database.lease.handle())
		.await
		.unwrap()
}

async fn pending_run(node: &Node, id: Uuid, tool_calls: Value) {
	let mut run = run_record(node, id).await;
	run.phase = RunPhase::ToolCall;
	run.pending = crate::state::tool_pending(json!({
		"response":{"text":"one committed result","tool_calls":tool_calls,
			"input_tokens":0,"output_tokens":0},"cursor":0
	}))
	.into();
	Run::objects()
		.update_with_conn(&mut node.database.lease.handle(), &run)
		.await
		.unwrap();
}

async fn steps(node: &Node, id: Uuid, count: usize) {
	for _ in 0..count {
		coordinator::advance(&node.f, id).await.unwrap();
	}
}
async fn complete(node: &Node, id: Uuid) -> Status {
	for _ in 0..40 {
		let state = coordinator::advance(&node.f, id).await.unwrap();
		if state.complete {
			return state;
		}
	}
	panic!("transaction did not finish: {:?}", node.status(id).await);
}
async fn unavailable(node: &Node, workspace_id: Uuid) {
	for path in [
		"/api/state".to_string(),
		"/api/registry".into(),
		format!("/api/workspaces/{workspace_id}"),
		"/api/events".into(),
		"/.well-known/aidash".into(),
	] {
		assert_eq!(node.get(&path).await.0, 503, "{path}");
	}
	assert_eq!(node.get("/health").await.0, 200);
	let (session_status, session) = node.get("/api/session").await;
	assert_eq!(session_status, 200);
	assert_eq!(session["access"]["kind"], "operator");
	assert_eq!(node.get("/api/transactions/participants").await.0, 200);
	let worker = Harness {
		federation: node.f.clone(),
	}
	.worker_once()
	.await;
	assert!(
		matches!(worker, Err(Error::TransactionPending)),
		"worker result: {worker:?}"
	);
	let provisioned = provision::reconcile(&node.f).await;
	assert!(
		matches!(provisioned, Err(Error::TransactionPending)),
		"provision result: {provisioned:?}"
	);
	assert!(
		node.f
			.store
			.update_state(workspace_id, 0, json!({"bypass":true}))
			.await
			.is_err(),
		"pending atomic work must reject a competing write"
	);
}

#[rstest]
#[tokio::test]
async fn abort_records_a_durable_decision_while_recovery_owns_the_transition_lease(
	#[future] pair: Option<Pair>,
) {
	let Some((a, b, manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	a.submit(&manifest).await;
	steps(&a, manifest.id, 4).await; // Both votes are prepared, still undecided.
	let mut transition = a.database.connection.begin().await.unwrap();
	let key = SimpleExpr::FunctionCall(
		Alias::new("hashtextextended").into_iden(),
		vec![
			Expr::value(format!("atomic:{}", manifest.id)).into(),
			Expr::value(0_i64).into(),
		],
	);
	let (sql, values) = Query::select()
		.expr(SimpleExpr::FunctionCall(
			Alias::new("pg_advisory_xact_lock").into_iden(),
			vec![key],
		))
		.build(PostgresQueryBuilder);
	transition
		.execute(&sql, convert_values(values))
		.await
		.unwrap();
	// A recovery step can hold this lease during slow peer I/O. An operator's
	// durable abort must not be lost merely because that step is in flight.
	let (status, body) = a
		.post(
			&format!("/api/transactions/{}/abort", manifest.id),
			&json!({}),
		)
		.await;
	transition.commit().await.unwrap();
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["decision"], "ABORT");
	assert!(!body["complete"].as_bool().unwrap());
	assert_eq!(
		coordinator::abort(&a.f, manifest.id)
			.await
			.unwrap()
			.decision
			.as_deref(),
		Some("ABORT")
	);
	let final_state = complete(&a, manifest.id).await;
	assert_eq!(final_state.decision.as_deref(), Some("ABORT"));
	assert_eq!(workspace(&a, wa).await.state.0, json!({}));
	assert_eq!(workspace(&b, wb).await.state.0, json!({}));
	let history = AtomicHistory::objects()
		.filter(AtomicHistory::field_transaction_id().eq(manifest.id))
		.all_with_db(&mut a.database.lease.handle())
		.await
		.unwrap();
	let decisions = history
		.iter()
		.filter(|row| {
			row.role == AtomicHistoryRole::Coordinator
				&& matches!(row.phase.as_str(), "COMMIT" | "ABORT")
		})
		.count();
	assert_eq!(decisions, 1);
}

#[rstest]
#[tokio::test]
async fn two_node_commit_hides_partial_application_and_releases_only_after_all_apply(
	#[future] pair: Option<Pair>,
) {
	let Some((a, b, manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	a.submit(&manifest).await;
	steps(&a, manifest.id, 2).await;
	unavailable(&a, wa).await;
	unavailable(&b, wb).await;
	assert!(matches!(
		a.f.request::<Value>(
			&b.f.config.node_id,
			reqwest::Method::POST,
			"/discover",
			Some(&json!({}))
		)
		.await,
		Err(Error::TransactionPending)
	));
	steps(&a, manifest.id, 2).await; // Both prepared; speculative writes rolled back.
	assert_eq!(workspace(&a, wa).await.state.0, json!({}));
	assert_eq!(workspace(&b, wb).await.state.0, json!({}));
	steps(&a, manifest.id, 1).await; // Durable commit precedes any application.
	assert_eq!(
		a.status(manifest.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	assert!(coordinator::abort(&a.f, manifest.id).await.is_err());
	assert!(
		!coordinator_records::transition(
			a.database.lease.handle(),
			manifest.id,
			CoordinatorTransition::Decide(AtomicCoordinatorDecision::Abort),
			"late conflicting decision"
		)
		.await
		.unwrap()
	);
	let stored = AtomicCoordinator::objects()
		.filter(AtomicCoordinator::field_id().eq(manifest.id))
		.get_with_db(&mut a.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(stored.decision, Some(AtomicCoordinatorDecision::Commit));
	steps(&a, manifest.id, 1).await;
	assert_eq!(workspace(&a, wa).await.state.0["value"], "new-a");
	assert_eq!(workspace(&b, wb).await.state.0, json!({}));
	unavailable(&a, wa).await;
	unavailable(&b, wb).await;
	steps(&a, manifest.id, 1).await;
	assert_eq!(workspace(&b, wb).await.state.0["value"], "new-b");
	assert!(!a.status(manifest.id).await.visible);
	steps(&a, manifest.id, 1).await; // Visibility certificate, barriers still retained.
	assert!(a.status(manifest.id).await.visible);
	steps(&a, manifest.id, 1).await;
	assert_eq!(a.get(&format!("/api/workspaces/{wa}")).await.0, 200);
	assert_eq!(b.get(&format!("/api/workspaces/{wb}")).await.0, 503);
	let done = complete(&a, manifest.id).await;
	assert!(done.visible);
	for (node, workspace_id) in [(&a, wa), (&b, wb)] {
		assert_eq!(
			node.get(&format!("/api/workspaces/{workspace_id}")).await.0,
			200
		);
		assert_eq!(
			participant::finish(&node.f, &a.f.config.node_id, &manifest)
				.await
				.unwrap()
				.phase,
			"COMMITTED"
		);
		assert_eq!(updates(node, workspace_id).await, 1);
	}
}

#[rstest]
#[tokio::test]
async fn stale_prepare_aborts_every_node_and_delayed_reserve_cannot_resurrect_it(
	#[future] pair: Option<Pair>,
) {
	let Some((a, b, mut manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	if let Mutation::WorkspaceState {
		expected_revision, ..
	} = &mut manifest.participants[1].mutations[0]
	{
		*expected_revision = 9;
	}
	a.submit(&manifest).await;
	let done = complete(&a, manifest.id).await;
	assert_eq!(done.decision.as_deref(), Some("ABORT"));
	assert!(!done.visible);
	for (node, workspace_id) in [(&a, wa), (&b, wb)] {
		assert_eq!(workspace(node, workspace_id).await.state.0, json!({}));
		assert_eq!(
			node.get(&format!("/api/workspaces/{workspace_id}")).await.0,
			200
		);
		assert_eq!(
			participant::reserve(&node.f, &a.f.config.node_id, &manifest)
				.await
				.unwrap()
				.phase,
			"ABORTED"
		);
		assert_eq!(
			participant::finish(&node.f, &a.f.config.node_id, &manifest)
				.await
				.unwrap()
				.phase,
			"ABORTED"
		);
	}
}

#[rstest]
#[tokio::test]
async fn partition_after_commit_retains_barriers_and_restart_recovers_the_same_decision(
	#[future] pair: Option<Pair>,
) {
	let Some((mut a, mut b, manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	a.submit(&manifest).await;
	steps(&a, manifest.id, 5).await;
	b.stop().await;
	steps(&a, manifest.id, 2).await;
	let waiting = a.status(manifest.id).await;
	assert_eq!(waiting.decision.as_deref(), Some("COMMIT"));
	assert!(!waiting.visible);
	assert!(waiting.last_error.is_some());
	unavailable(&a, wa).await;
	a.restart().await;
	b.restart().await;
	unavailable(&a, wa).await;
	unavailable(&b, wb).await;
	let done = complete(&a, manifest.id).await;
	assert_eq!(done.decision.as_deref(), Some("COMMIT"));
	assert_eq!(workspace(&a, wa).await.revision, 1);
	assert_eq!(workspace(&b, wb).await.revision, 1);
}

#[rstest]
#[tokio::test]
async fn overlapping_coordinators_use_the_same_node_order_without_lost_updates(
	#[future] pair: Option<Pair>,
) {
	let Some((a, b, first, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	let mut second = first.clone();
	second.id = Uuid::new_v4();
	second.coordinator = b.f.config.node_id.clone();
	a.submit(&first).await;
	b.submit(&second).await;
	steps(&a, first.id, 1).await;
	steps(&b, second.id, 1).await;
	let waiting = b.status(second.id).await;
	assert!(waiting.decision.is_none());
	assert!(waiting.last_error.is_some());
	assert_eq!(
		complete(&a, first.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	assert_eq!(
		complete(&b, second.id).await.decision.as_deref(),
		Some("ABORT")
	);
	assert_eq!(workspace(&a, wa).await.revision, 1);
	assert_eq!(workspace(&b, wb).await.revision, 1);
}

#[rstest]
#[tokio::test]
async fn registry_workspace_task_execution_and_artifact_commit_together_once(
	#[future] pair: Option<Pair>,
) {
	use aidash_server::{
		domain::{NewTask, qualified_agent},
		registry::Entry,
	};
	let Some((a, b, mut manifest, wa, _wb)) = Box::pin(pair).await else {
		return;
	};
	let agent:Entry=serde_json::from_value(json!({"id":"executor","version":"1.0.0","kind":"agent","name":{"en":"Executor"},"description":{"en":"Atomic fixture"},"config":{"model":{"id":"fixture","version":"1.0.0"},"instructions":"Atomic execution","tools":[],"skills":[]}})).unwrap();
	let owner = qualified_agent(&b.f.config.node_id, &agent.id, &agent.version);
	let task =
		a.f.store
			.create_task(
				wa,
				&NewTask {
					title: "Atomic task".into(),
					description: "Finish with the remote execution".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"operator",
				None,
			)
			.await
			.unwrap();
	a.f.store.claim(task.id, 0, &owner, &agent).await.unwrap();
	let task =
		a.f.store
			.transition(
				task.id,
				1,
				&owner,
				aidash_server::domain::TaskStatus::Running,
			)
			.await
			.unwrap();
	let delegation = Delegation::build()
		.task_id(task.id)
		.node_id(&b.f.config.node_id)
		.agent_id(&agent.id)
		.agent_version(&agent.version)
		.delivered(true)
		.finish();
	Delegation::objects()
		.create_with_conn(&mut a.database.lease.handle(), &delegation)
		.await
		.unwrap();
	let run =
		b.f.store
			.accept_run(&task, &a.f.config.node_id, &agent.id, &agent.version)
			.await
			.unwrap();
	pending_run(&b, run.id, json!([])).await;
	manifest.participants[0].mutations.push(serde_json::from_value(json!({"kind":"registry_register","entry":{"id":"atomic-skill","version":"1.0.0","kind":"skill","name":{"en":"Atomic skill"},"description":{"en":"Prepared registration"},"config":{"instructions":"Complete atomic work"}}})).unwrap());
	manifest.participants[0].mutations.push(serde_json::from_value(json!({"kind":"complete_task","task_id":task.id,"expected_revision":task.revision,"artifact":{"kind":"text","name":"Atomic result","content":"one committed result"}})).unwrap());
	manifest.participants[1].mutations.push(
		serde_json::from_value(
			json!({"kind":"finish_run","run_id":run.id,"task_id":task.id,"expected_revision":run.revision}),
		)
		.unwrap(),
	);
	let mut invalid = manifest.clone();
	invalid.id = Uuid::new_v4();
	pending_run(
		&b,
		run.id,
		json!([{"id":"unfinished","name":"http","arguments":{}}]),
	)
	.await;
	a.submit(&invalid).await;
	assert_eq!(
		complete(&a, invalid.id).await.decision.as_deref(),
		Some("ABORT")
	);
	assert_eq!(
		a.f.store.task(task.id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Running
	);
	assert!(a.f.store.snapshot(wa).await.unwrap().artifacts.is_empty());
	pending_run(&b, run.id, json!([])).await;
	a.submit(&manifest).await;
	assert_eq!(
		complete(&a, manifest.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	for _ in 0..2 {
		participant::finish(&a.f, &a.f.config.node_id, &manifest)
			.await
			.unwrap();
		participant::finish(&b.f, &a.f.config.node_id, &manifest)
			.await
			.unwrap();
	}
	assert_eq!(
		a.f.store.task(task.id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Completed
	);
	assert_eq!(
		b.f.store.run(run.id).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
	assert_eq!(
		a.f.registry
			.get("atomic-skill", "1.0.0")
			.await
			.unwrap()
			.kind,
		"skill"
	);
	let artifacts = a.f.store.snapshot(wa).await.unwrap().artifacts;
	assert_eq!(artifacts.len(), 1);
	assert_eq!(artifacts[0].created_by, owner);
}

#[rstest]
#[tokio::test]
async fn participant_pulls_only_durable_decisions_and_never_guesses_after_timeout(
	#[future] pair: Option<Pair>,
) {
	let Some((mut a, b, manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	a.submit(&manifest).await;
	steps(&a, manifest.id, 4).await;
	a.stop().await;
	assert_eq!(participant::recover_once(&b.f).await.unwrap(), 0);
	assert_eq!(workspace(&b, wb).await.revision, 0);
	unavailable(&b, wb).await;
	a.restart().await;
	steps(&a, manifest.id, 1).await;
	// The coordinator has not sent apply, but a participant can recover its vote.
	assert_eq!(participant::recover_once(&b.f).await.unwrap(), 1);
	assert_eq!(workspace(&b, wb).await.revision, 1);
	unavailable(&a, wa).await;
	unavailable(&b, wb).await;
	assert_eq!(
		complete(&a, manifest.id).await.decision.as_deref(),
		Some("COMMIT")
	);
}

#[rstest]
#[tokio::test]
async fn an_existing_sse_stream_waits_for_atomic_visibility_before_emitting_changes(
	#[future] pair: Option<Pair>,
) {
	let Some((a, b, manifest, wa, _wb)) = Box::pin(pair).await else {
		return;
	};
	let mut response = http_client()
		.get(format!(
			"{}/api/events/stream?workspace_id={wa}",
			a.f.config.endpoint
		))
		.bearer_auth(&a.f.config.api_token)
		.send()
		.await
		.unwrap();
	assert_eq!(response.status(), 200);
	assert_eq!(response.headers()["content-type"], "text/event-stream");
	let first = next_frame(&mut response).await;
	assert!(first.contains("workspace.created"), "{first}");
	a.submit(&manifest).await;
	steps(&a, manifest.id, 6).await;
	assert!(
		tokio::time::timeout(std::time::Duration::from_millis(300), response.chunk())
			.await
			.is_err()
	);
	complete(&a, manifest.id).await;
	let event = next_frame(&mut response).await;
	assert!(event.contains("new-a"), "{event}");
	assert!(!event.contains("new-b"), "{event}");
	drop(response);
	drop(b);
}

#[rstest]
#[tokio::test]
async fn peer_trust_denial_aborts_promptly_and_revocation_preserves_admitted_recovery(
	#[future] pair: Option<Pair>,
) {
	let Some((a, b, manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	let set_trust = |enabled| json!({"node_id":a.f.config.node_id,"enabled":enabled});
	assert_eq!(
		b.post("/api/transactions/trust", &set_trust(false)).await.0,
		200
	);
	a.submit(&manifest).await;
	steps(&a, manifest.id, 2).await;
	// A terminal peer rejection must abort immediately, without waiting for the deadline.
	assert_eq!(
		a.status(manifest.id).await.decision.as_deref(),
		Some("ABORT")
	);
	complete(&a, manifest.id).await;
	assert_eq!(workspace(&a, wa).await.revision, 0);
	assert_eq!(workspace(&b, wb).await.revision, 0);

	assert_eq!(
		b.post("/api/transactions/trust", &set_trust(true)).await.0,
		200
	);
	let mut admitted = manifest.clone();
	admitted.id = Uuid::new_v4();
	// An authenticated peer cannot claim that a different node coordinates its manifest.
	let mut forged = admitted.clone();
	forged.coordinator = b.f.config.node_id.clone();
	let client = &b.peer_client;
	let peer_headers = [
		(
			"authorization",
			format!("Bearer {}", crate::fixtures::PEER_SECRET),
		),
		("x-aidash-node", a.f.config.node_id.clone()),
		("x-aidash-protocol", "0.1".into()),
	];
	let peer_headers: Vec<_> = peer_headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
	let response = client
		.post_raw_with_headers(
			"/federation/v0.1/transactions/reserve",
			&serde_json::to_vec(&forged).unwrap(),
			"application/json",
			&peer_headers,
		)
		.await
		.unwrap();
	assert_eq!(response.status_code(), 403, "{}", response.text());
	assert_eq!(b.get(&format!("/api/workspaces/{wb}")).await.0, 200);
	a.submit(&admitted).await;
	steps(&a, admitted.id, 2).await;
	assert_eq!(
		b.post("/api/transactions/trust", &set_trust(false)).await.0,
		200
	);
	assert_eq!(
		complete(&a, admitted.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	assert_eq!(workspace(&a, wa).await.revision, 1);
	assert_eq!(workspace(&b, wb).await.revision, 1);
}

#[rstest]
#[tokio::test]
async fn every_durable_transition_survives_fresh_pools_and_http_servers(
	#[future] pair: Option<Pair>,
) {
	let Some((mut a, mut b, manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	a.submit(&manifest).await;
	let mut transitions = 0;
	loop {
		a.restart().await;
		b.restart().await;
		let state = coordinator::advance(&a.f, manifest.id).await.unwrap();
		transitions += 1;
		if !state.visible {
			// No node may expose a new value before the global visibility decision.
			for (node, workspace_id) in [(&a, wa), (&b, wb)] {
				let (status, value) = node.get(&format!("/api/workspaces/{workspace_id}")).await;
				assert!(
					status == 503 || (status == 200 && value["workspace"]["revision"] == 0),
					"{status}: {value}"
				);
			}
		}
		if state.complete {
			break;
		}
		assert!(transitions < 20);
	}
	assert_eq!(transitions, 11);
	assert_eq!(workspace(&a, wa).await.revision, 1);
	assert_eq!(workspace(&b, wb).await.revision, 1);
}

#[rstest]
#[tokio::test]
async fn undecided_deadline_aborts_without_publishing_prepared_mutations(
	#[future] pair: Option<Pair>,
) {
	let Some((a, b, mut manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	manifest.deadline = Utc::now() + Duration::seconds(2);
	a.submit(&manifest).await;
	steps(&a, manifest.id, 4).await;
	let remaining = (manifest.deadline - Utc::now())
		.to_std()
		.unwrap_or_default();
	tokio::time::sleep(remaining + std::time::Duration::from_millis(10)).await;
	assert_eq!(
		complete(&a, manifest.id).await.decision.as_deref(),
		Some("ABORT")
	);
	assert_eq!(workspace(&a, wa).await.revision, 0);
	assert_eq!(workspace(&b, wb).await.revision, 0);
}

#[rstest]
#[tokio::test]
async fn actual_worker_sigkill_after_commit_recovers_without_replaying_effects(
	#[future] pair: Option<Pair>,
	#[from(reinhardt::test::fixtures::temp_dir)] worker_directory: tempfile::TempDir,
	#[from(reinhardt::test::fixtures::temp_dir)] restart_a_directory: tempfile::TempDir,
	#[from(reinhardt::test::fixtures::temp_dir)] restart_b_directory: tempfile::TempDir,
) {
	let Some((a, mut b, manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	a.submit(&manifest).await;
	steps(&a, manifest.id, 5).await;
	b.stop().await;
	// Act: start recovery after submitting the committed transaction and partitioning its peer.
	let mut worker = WorkerProcess::start(&a, worker_directory).await;
	tokio::time::timeout(std::time::Duration::from_secs(15), async {
		loop {
			worker.assert_running();
			let state = AtomicParticipant::objects()
				.filter(AtomicParticipant::field_id().eq(manifest.id))
				.get_with_db(&mut a.database.lease.handle())
				.await
				.unwrap();
			if state.phase == AtomicParticipantPhase::Applied {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.unwrap_or_else(|error| {
		panic!(
			"worker did not apply the committed transaction: {error}; {}",
			worker.log()
		)
	});
	worker.kill().await; // SIGKILL the actual recovery loops after durable local application.
	assert_eq!(
		a.status(manifest.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	unavailable(&a, wa).await;
	b.restart().await;
	// Act: replace killed workers with fresh declared process directories.
	let mut restarted_a = WorkerProcess::start(&a, restart_a_directory).await;
	let mut restarted_b = WorkerProcess::start(&b, restart_b_directory).await;
	tokio::time::timeout(std::time::Duration::from_secs(15), async {
		loop {
			restarted_a.assert_running();
			restarted_b.assert_running();
			if a.status(manifest.id).await.complete {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.unwrap_or_else(|error| {
		panic!(
			"worker recovery did not finish: {error}; node A: {}; node B: {}",
			restarted_a.log(),
			restarted_b.log()
		)
	});
	restarted_a.kill().await;
	restarted_b.kill().await;
	for (node, workspace_id) in [(&a, wa), (&b, wb)] {
		assert_eq!(workspace(node, workspace_id).await.revision, 1);
		assert_eq!(updates(node, workspace_id).await, 1);
	}
}

#[rstest]
#[tokio::test]
async fn unreachable_aborted_transactions_cannot_starve_later_local_work(
	#[future] pair: Option<Pair>,
) {
	let Some((a, mut b, manifest, wa, _wb)) = Box::pin(pair).await else {
		return;
	};
	for _ in 0..33 {
		let mut old = manifest.clone();
		old.id = Uuid::new_v4();
		a.submit(&old).await;
		coordinator::abort(&a.f, old.id).await.unwrap();
		steps(&a, old.id, 1).await;
	}
	b.stop().await;
	let mut local = manifest.clone();
	local.id = Uuid::new_v4();
	local.participants.truncate(1);
	a.submit(&local).await;
	for _ in 0..20 {
		coordinator::recover_once(&a.f).await.unwrap();
		if a.status(local.id).await.complete {
			break;
		}
	}
	assert!(a.status(local.id).await.complete);
	assert_eq!(workspace(&a, wa).await.revision, 1);
}

async fn next_frame(response: &mut reqwest::Response) -> String {
	tokio::time::timeout(std::time::Duration::from_secs(5), async {
		let mut bytes = Vec::new();
		loop {
			let chunk = response.chunk().await.unwrap().expect("SSE remains open");
			bytes.extend_from_slice(&chunk);
			if bytes.ends_with(b"\n\n") {
				return String::from_utf8(bytes).unwrap();
			}
			assert!(bytes.len() < 65536);
		}
	})
	.await
	.expect("SSE frame deadline")
}
