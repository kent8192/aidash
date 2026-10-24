//! Distributed transaction behavior over production Reinhardt routes.
use crate::fixtures::{Node, Pair, WorkerProcess, pair};
use aidash_application::tenancy::TEST_FIXTURE;
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
	Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use rstest::rstest;
use serde_json::{Value, json};
use uuid::Uuid;

async fn workspace(node: &Node, id: Uuid) -> Workspace {
	node.database
		.fixture(async |tx| {
			Ok(Workspace::objects()
				.filter(Workspace::field_id().eq(id))
				.get_with_db(tx)
				.await?)
		})
		.await
}

async fn updates(node: &Node, id: Uuid) -> usize {
	node.database
		.fixture(async |tx| {
			Ok(Event::objects()
				.filter(Event::field_workspace_id().eq(Some(id)))
				.filter(Event::field_kind().eq("workspace.updated"))
				.all_with_db(tx)
				.await?
				.len())
		})
		.await
}

async fn run_record(node: &Node, id: Uuid) -> Run {
	node.database
		.fixture(async |tx| {
			Ok(Run::objects()
				.filter(Run::field_id().eq(id))
				.get_with_db(tx)
				.await?)
		})
		.await
}

async fn pending_run(node: &Node, id: Uuid, tool_calls: Value) {
	let mut run = run_record(node, id).await;
	run.phase = RunPhase::ToolCall;
	run.pending = crate::state::tool_pending(json!({
		"response":{"text":"one committed result","tool_calls":tool_calls,
			"input_tokens":0,"output_tokens":0},"cursor":0
	}))
	.into();
	node.database
		.fixture(async |tx| {
			Run::objects().update_with_conn(tx, &run).await?;
			Ok(())
		})
		.await;
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
	assert_eq!(session["access"]["kind"], "deployment_administrator");
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
			.update_state(&TEST_FIXTURE, workspace_id, 0, json!({"bypass":true}))
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
	// A recovery step can hold this lease during slow peer I/O. A Deployment Administrator's
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
			&a.sender(),
			b.address.as_str(),
			reqwest::Method::POST,
			"/scoped/discover",
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
			&a.f.store.control_pool,
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
			participant::finish(&node.f, None, a.address.as_str(), &manifest)
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
			participant::reserve(&node.f, None, a.address.as_str(), &manifest)
				.await
				.unwrap()
				.phase,
			"ABORTED"
		);
		assert_eq!(
			participant::finish(&node.f, None, a.address.as_str(), &manifest)
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
	second.coordinator = b.address.clone();
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
#[case::ordinary(false)]
#[case::node_deletion_after_decision(true)]
#[tokio::test]
async fn workspace_task_execution_and_artifact_commit_together_once(
	#[future] pair: Option<Pair>,
	#[case] delete_after_decision: bool,
) {
	use aidash_server::{
		domain::{NewTask, qualified_agent},
		registry::Entry,
	};
	let Some((a, b, mut manifest, wa, _wb)) = Box::pin(pair).await else {
		return;
	};
	let agent:Entry=serde_json::from_value(json!({"id":"executor","version":"1.0.0","kind":"agent","name":{"en":"Executor"},"description":{"en":"Atomic fixture"},"config":{"model":{"id":"fixture","version":"1.0.0"},"instructions":"Atomic execution","schema_version":1,"bindings":[],"remove_default":[]}})).unwrap();
	let model: Entry = serde_json::from_value(json!({"id":"fixture","version":"1.0.0","kind":"model","name":{"en":"Atomic model"},"description":{"en":"Pinned execution fixture"},"config":{"provider":"openrouter","model_id":"fixture","endpoint":"http://localhost:19999/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}})).unwrap();
	b.f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			model,
		)
		.await
		.unwrap();
	b.f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			agent.clone(),
		)
		.await
		.unwrap();

	let owner = qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(b.f.config.deployment.as_str(), "acme"),
		&agent.id,
		&agent.version,
	);
	let task =
		a.f.store
			.create_task(
				&TEST_FIXTURE,
				wa,
				&NewTask {
					title: "Atomic task".into(),
					description: "Finish with the remote execution".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"deployment_administrator",
				None,
			)
			.await
			.unwrap();
	a.f.store
		.claim(&TEST_FIXTURE, task.id, 0, &owner, &agent)
		.await
		.unwrap();
	let task =
		a.f.store
			.transition(
				&TEST_FIXTURE,
				task.id,
				1,
				&owner,
				aidash_server::domain::TaskStatus::Running,
			)
			.await
			.unwrap();
	let delegation = Delegation::build()
		.tenant("acme")
		.task_id(task.id)
		.node_id(b.address.as_str())
		.agent_id(&agent.id)
		.agent_version(&agent.version)
		.delivered(true)
		.human_requests(json!([]).into())
		.binding_snapshot(None)
		.finish();
	a.f.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			Delegation::objects()
				.create_with_conn(tx, &delegation)
				.await?;
			Ok(())
		})
		.await
		.unwrap();
	let run = crate::fixtures::tenant_roots::remote_run(
		&b.f.store,
		&task,
		a.address.as_str(),
		&agent.id,
		&agent.version,
	)
	.await;
	pending_run(&b, run.id, json!([])).await;
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
		a.f.store.task(&TEST_FIXTURE, task.id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Running
	);
	assert!(
		a.f.store
			.snapshot(&TEST_FIXTURE, wa)
			.await
			.unwrap()
			.artifacts
			.is_empty()
	);
	pending_run(&b, run.id, json!([])).await;
	if delete_after_decision {
		let sibling =
			crate::fixtures::tenant_roots::second_node_in(b.f.store.pool.fixture_driver(), "acme")
				.await;
		let (status, body) = b
			.post(
				&format!("/api/tenants/acme/nodes/{sibling}/default"),
				&json!(null),
			)
			.await;
		assert_eq!(status, 200, "{body}");
	}
	a.submit(&manifest).await;
	if delete_after_decision {
		steps(&a, manifest.id, 5).await;
		assert_eq!(
			a.status(manifest.id).await.decision.as_deref(),
			Some("COMMIT")
		);
		let before = run_record(&b, run.id).await;
		let target: aidash_domain::nodes::NodeAddress = b.address.parse().unwrap();
		let (status, body) = b
			.post(
				&format!("/api/tenants/acme/nodes/{}/deletion", target.node),
				&json!({"confirmation":target.node}),
			)
			.await;
		assert_eq!(status, 200, "{body}");
		let (status, body) = b
			.post(
				&format!("/api/tenants/acme/nodes/{}/restore", target.node),
				&json!(null),
			)
			.await;
		assert_eq!(status, 200, "{body}");
		let after = run_record(&b, run.id).await;
		assert_eq!(
			after.revision, before.revision,
			"deletion cannot invalidate a decided participant"
		);
		let mut fresh = manifest.clone();
		fresh.id = Uuid::new_v4();
		let rejected = participant::reserve(&b.f, None, a.address.as_str(), &fresh)
			.await
			.unwrap_err();
		assert!(
			matches!(rejected, aidash_server::Error::Conflict(ref reason) if reason == "Node cancellation is pending"),
			"restoration must not reopen a captured Run while its decision is recovering: {rejected}"
		);
	}
	assert_eq!(
		complete(&a, manifest.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	if delete_after_decision {
		provision::reconcile(&b.f).await.unwrap();
		let remaining: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("node_cancellation_jobs"))
				.and_where(Expr::col("kind").eq("run"))
				.and_where(Expr::col("work_id").eq(Expr::value(run.id)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(b.f.store.pool.fixture_driver())
		.await
		.unwrap();
		assert_eq!(
			remaining, 0,
			"terminal owner retires the captured job after recovery"
		);
		let mut fresh = manifest.clone();
		fresh.id = Uuid::new_v4();
		assert!(
			matches!(
				participant::reserve(&b.f, None, a.address.as_str(), &fresh).await,
				Err(aidash_server::Error::Forbidden)
			),
			"retiring the queue must not restore old Run authority"
		);
	}
	for _ in 0..2 {
		participant::finish(&a.f, None, a.address.as_str(), &manifest)
			.await
			.unwrap();
		participant::finish(&b.f, None, a.address.as_str(), &manifest)
			.await
			.unwrap();
	}
	assert_eq!(
		a.f.store.task(&TEST_FIXTURE, task.id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Completed
	);
	assert_eq!(
		b.f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"COMPLETED"
	);
	let artifacts =
		a.f.store
			.snapshot(&TEST_FIXTURE, wa)
			.await
			.unwrap()
			.artifacts;
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
	// reinhardt-web#6661: incremental SSE frames need the fixture-owned raw streaming client.
	let mut response =
		a.f.client
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
	// `b` refuses atomic transactions with `a`'s Node, then allows them again.
	assert_eq!(b.set_trust(&a, false, 1).await, 200);
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

	assert_eq!(b.set_trust(&a, true, 2).await, 200);
	let mut admitted = manifest.clone();
	admitted.id = Uuid::new_v4();
	// An authenticated peer cannot claim that a different node coordinates its manifest.
	let mut forged = admitted.clone();
	forged.coordinator = b.address.clone();
	let client = &b.peer_client;
	let body = serde_json::to_vec(&forged).unwrap();
	let token = crate::fixtures::peering::token(
		a.f.config.deployment_key.as_deref().unwrap(),
		a.f.config.deployment.as_str(),
		&a.address,
		&b.address,
		"",
		("POST", "/transactions/reserve", &body),
	)
	.await;
	let peer_headers = crate::fixtures::peering::peer_headers(&token);
	let peer_headers: Vec<_> = peer_headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
	let response = async {
		let client = &(client);
		let mut request = client
			.request(http::Method::POST, "/federation/v0.1/transactions/reserve")
			.body(bytes::Bytes::copy_from_slice(
				&serde_json::to_vec(&forged).unwrap(),
			))
			.header(http::header::CONTENT_TYPE, "application/json");
		for (name, value) in &peer_headers {
			request = request.header(*name, *value);
		}
		request.send().await
	}
	.await
	.unwrap();
	assert_eq!(response.status_code(), 403, "{}", response.text());
	assert_eq!(b.get(&format!("/api/workspaces/{wb}")).await.0, 200);
	a.submit(&admitted).await;
	steps(&a, admitted.id, 2).await;
	// The registration is guarded like any other Tenant data: while `b` holds
	// the transaction's barrier it cannot be revoked, so the admitted
	// transaction recovers and commits.
	assert_eq!(b.set_trust(&a, false, 3).await, 503);
	assert_eq!(
		complete(&a, admitted.id).await.decision.as_deref(),
		Some("COMMIT")
	);
	assert_eq!(workspace(&a, wa).await.revision, 1);
	assert_eq!(workspace(&b, wb).await.revision, 1);
	assert_eq!(b.set_trust(&a, false, 3).await, 200);
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

#[path = "node_purge.rs"]
mod node_purge_recovery;
