//! Issues #291 and #332: manual retries renew only the retry ledger (ADR 0057).
//! A library test, because a Run's memory recall goes through crate-private
//! authority.
use super::fixture::*;
use super::upstream::{self, reply};
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::apps::knowledge::services::native_memory as memory;
use crate::authorization::identity::Actor;
use crate::store::Store;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::memory::*;
use reinhardt::ServerRouter as Router;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use rstest::{fixture, rstest};
use serde_json::json;
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

struct RuntimeControls {
	client: reqwest::Client,
	notify: Arc<tokio::sync::Notify>,
}
#[fixture]
fn runtime_controls(
	#[from(reinhardt::test::fixtures::http_client)] client: reqwest::Client,
) -> RuntimeControls {
	RuntimeControls {
		client,
		notify: Arc::new(tokio::sync::Notify::new()),
	}
}

#[derive(Clone)]
struct OutageState {
	down: Arc<AtomicBool>,
}
#[fixture]
fn outage_state() -> OutageState {
	OutageState {
		down: Arc::new(AtomicBool::new(false)),
	}
}
#[fixture]
fn outage_router(#[from(outage_state)] state: OutageState) -> Arc<Router> {
	let down = state.down.clone();
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/v1/embeddings",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let input = request.json::<serde_json::Value>().unwrap();
					let down = down.load(Ordering::Acquire);
					async move {
						if down {
							return reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE)
								.with_json(&json!({"error":"fixture outage"}))
								.unwrap();
						}
						reinhardt::Response::ok().with_json(&json!({"model":input["model"],"data":[{"index":0,"embedding":[1.0,0.1,0.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}})).unwrap()
					}
				}),
			)
			.into_server_router(),
	)
}
struct OutageProvider {
	state: OutageState,
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn outage_provider(
	#[from(outage_state)] state: OutageState,
	#[from(outage_router)]
	#[with(state.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> OutageProvider {
	OutageProvider { state, server }
}

const KEY: &str = "retry-cycle-recall";

/// Recall through a Run's memory tool. The same key replays the same memory
/// operation, as an Ordered Run's Retrieval Key does for its context recall.
async fn run_recall(
	store: &Store,
	run: &aidash_domain::Run,
) -> aidash_server::Result<memory::Outcome> {
	let access =
		aidash_server::authorization::execution::access_for_run(store, &run.metadata(), true)
			.await?
			.ok_or(aidash_server::Error::Forbidden)?;
	let mut lease = aidash_server::semantic::service::Lease::Scoped(Box::new(access));
	let result = memory::run_recall(
		store,
		&mut lease,
		run,
		KEY,
		RecallQuery {
			text: "subway".into(),
			time: None,
			kinds: vec![],
			max_tokens: 8192,
		},
		false,
	)
	.await;
	lease.finish(result).await
}

/// Move every attempt past its call deadline, so a replay makes a new one.
async fn expire_attempts(store: &Store) {
	aidash_server::database::native::query(
		&Query::update()
			.table(Alias::new("memory_model_attempts"))
			.value(
				Alias::new("created_at"),
				chrono::Utc::now() - chrono::Duration::hours(1),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
}

/// The recall operation's attempts per Retry Cycle and their states, oldest first.
async fn attempts(store: &Store, operation: Uuid) -> Vec<(i32, String)> {
	aidash_server::database::native::query(
		&Query::select()
			.columns(["cycle", "state"].map(Alias::new))
			.from(Alias::new("memory_model_attempts"))
			.and_where(Expr::col("operation_id").eq(Expr::value(operation)))
			.order_by(Alias::new("created_at"), reinhardt::query::Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap()
	.into_iter()
	.map(|row| {
		(
			row.try_get::<i32>("cycle").unwrap(),
			row.try_get::<String>("state").unwrap(),
		)
	})
	.collect()
}

fn failure(
	result: &aidash_server::Result<memory::Outcome>,
) -> Option<aidash_domain::semantic::Failure> {
	match result {
		Err(aidash_server::Error::RemoteSemantic(reason)) => Some(*reason),
		_ => None,
	}
}

/// A Run whose memory retry ledger an embedding outage spent pauses as
/// `RetriesExhausted`; it stays spent after the provider recovers until an
/// authorized manual retry opens the Run's next Retry Cycle. The new cycle
/// keeps the operation ID and its durable totals, and later replays are
/// served from the ledger.
#[rstest]
#[case::recovered(true, false)]
#[case::durable_budget(false, false)]
#[case::durable_token_budget(false, true)]
#[tokio::test]
async fn manual_retry_renews_only_the_retry_ledger_and_preserves_durable_budgets(
	#[case] provider_recovers: bool,
	#[case] token_budget: bool,
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
	#[future(awt)] outage_provider: OutageProvider,
	runtime_controls: RuntimeControls,
) {
	use aidash_domain::{NewTask, qualified_agent};
	use aidash_server::{
		authorization::Authorization, config::Config, domain::RunControlAction,
		federation::Federation,
	};
	// Arrange: a claimed Run with a Home-issued participant Bank holding one Unit.
	let provider = outage_provider.server;
	let down = outage_provider.state.down;
	let endpoint = format!("{}/v1", provider.url);
	let database = database.await;
	if token_budget {
		bounds.max_model_calls = 8;
		bounds.max_model_tokens = 24; // Four conservatively charged six-byte queries.
	}
	let (store, registry, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let agent = reference("a");
	let authorization = Authorization {
		pool: store.pool.clone(),
	};
	let owner = qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme"),
		&agent.id,
		&agent.version,
	);
	let policy = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},owner:{"kind":"agent"}},"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]});
	authorization
		.replace(
			"acme",
			1,
			serde_json::from_value(policy).unwrap(),
			"deployment_administrator",
		)
		.await
		.unwrap();
	for id in ["m", "e", "r", "t", "p", "a"] {
		authorization
			.set_catalog(
				"acme",
				&aidash_domain::nodes::NodeName::derived("acme"),
				&reference(id),
				0,
				true,
				"deployment_administrator",
			)
			.await
			.unwrap();
	}
	let issued = authorization
		.issue_credential("acme", "alice", 3600, "deployment_administrator")
		.await
		.unwrap();
	let actor = authorization.authenticate(&issued.token).await.unwrap();
	let Actor::Subject(identity) = &actor else {
		panic!("subject fixture");
	};
	let participant = memory::create_participant(
		&store,
		&actor,
		workspace,
		memory::CreateParticipant {
			agent: agent.clone(),
		},
	)
	.await
	.unwrap();
	memory::mutate(
		&store,
		&actor,
		mutation(
			&participant.bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Use the subway in Tokyo."),
			},
		),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let task = store
		.create_task(
			&TEST_FIXTURE,
			workspace,
			&NewTask {
				title: "Tokyo transport".into(),
				description: "Find the way around".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"alice",
			None,
		)
		.await
		.unwrap();
	memory::assign_participant(
		&store,
		&actor,
		workspace,
		task.id,
		memory::AssignmentChange {
			task_revision: task.revision,
			expected: None,
			target: Some(memory::AssignedParticipant {
				participant_id: participant.id,
				participant_revision: participant.revision,
			}),
		},
	)
	.await
	.unwrap();
	let f = Federation {
		sandbox: Default::default(),
		registry,
		config: Config {
			tenancy: aidash_domain::identity::tenancy::TenancyRules::of(
				aidash_domain::offering::Offering::SelfHosted,
			),
			deployment: store.deployment.clone(),
			endpoint: "http://127.0.0.1:1".into(),
			database_url: database.url.clone(),
			nats_url: "nats://127.0.0.1:1".into(),
			api_token: "fixture-deployment-administrator".into(),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
			demo_data: false,
			oidc: None,
			federation: true,
			deployment_key: None,
			sign_in: None,
			tier_limits: std::sync::Arc::new(aidash_application::ports::nodes::NoTierLimits),
		},
		store,
		client: runtime_controls.client,
		notify: runtime_controls.notify,
	};
	aidash_server::authorization::execution::claim(&f, identity, task.id, task.revision, &agent)
		.await
		.unwrap();
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let operation = memory::request_id(run.id, KEY).unwrap();

	// Act: an embedding outage spends cycle 0's ledger (`max_retries: 2`). The
	// replay inside the failed attempt's deadline waits without spending it.
	down.store(true, Ordering::Release);
	let mut outcomes = vec![failure(&run_recall(&f.store, &run).await)];
	outcomes.push(failure(&run_recall(&f.store, &run).await));
	for _ in 0..2 {
		expire_attempts(&f.store).await;
		outcomes.push(failure(&run_recall(&f.store, &run).await));
	}
	// Assert
	use aidash_domain::semantic::Failure;
	assert_eq!(
		outcomes,
		[
			Some(Failure::Unavailable),
			Some(Failure::Pending),
			Some(Failure::Unavailable),
			Some(Failure::RetriesExhausted),
		]
	);

	// Act: the provider recovers, but the Run is still in cycle 0.
	down.store(!provider_recovers, Ordering::Release);
	let spent = run_recall(&f.store, &run).await;
	// Assert: this was the dead end of #291.
	assert_eq!(
		failure(&spent),
		Some(Failure::RetriesExhausted),
		"{spent:?}"
	);

	// Act: deliver the real recall failure through worker recovery, then retry.
	recover(&f.store, run.id, spent.unwrap_err()).await;
	let resumed = f
		.store
		.control(&TEST_FIXTURE, run.id, RunControlAction::Resume)
		.await
		.unwrap();
	let run = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
	let recalled = run_recall(&f.store, &run).await;
	let replayed = run_recall(&f.store, &run).await;
	// Assert: the manual retry opened cycle 1 and was audited.
	assert_eq!(resumed.control.as_str(), "ACTIVE");
	let cycle: i32 = aidash_server::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("semantic_retry_cycle"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(f.store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(cycle, 1);
	let audit: serde_json::Value = aidash_server::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("events"))
			.and_where(Expr::col("kind").eq("semantic.manual_retry"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(f.store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(
		audit,
		json!({"run_id":run.id,"cycle":1,"cause":"retries_exhausted","reason":"authorized_resume"})
	);
	if !provider_recovers {
		assert_eq!(failure(&recalled), Some(Failure::Unavailable));
		assert_eq!(failure(&replayed), Some(Failure::Pending));
		expire_attempts(&f.store).await;
		assert_eq!(
			failure(&run_recall(&f.store, &run).await),
			Some(Failure::Unavailable)
		);
		expire_attempts(&f.store).await;
		let spent = run_recall(&f.store, &run).await.unwrap_err();
		recover(&f.store, run.id, spent).await;
		f.store
			.control(&TEST_FIXTURE, run.id, RunControlAction::Resume)
			.await
			.unwrap();
		let run = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
		down.store(false, Ordering::Release);
		let exhausted = run_recall(&f.store, &run).await;
		assert_eq!(
			serde_json::to_value(failure(&exhausted)).unwrap(),
			"durable_memory_budget",
			"{exhausted:?}"
		);
		recover(&f.store, run.id, exhausted.unwrap_err()).await;
		let stopped = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
		assert_eq!(stopped.control.as_str(), "PAUSED");
		assert!(stopped.recovery.retry.is_none());
		assert_ne!(
			f.store.task(&TEST_FIXTURE, task.id).await.unwrap().status,
			aidash_domain::TaskStatus::Failed
		);
		let details = crate::apps::identity::services::execution::management::get(
			f.clone(),
			actor.clone(),
			run.id,
		)
		.await
		.unwrap();
		assert_eq!(
			serde_json::to_value(details.semantic_recovery).unwrap(),
			json!({"cause":"durable_memory_budget","actions":["create_follow_up_task"]})
		);
		assert_eq!(attempts(&f.store, operation).await.len(), 4);
		return;
	}
	// Assert: the same operation recalls in cycle 1, and its replay is served
	// from the ledger without another attempt.
	for result in [&recalled, &replayed] {
		assert!(
			matches!(
				result,
				Ok(memory::Outcome::Recall(aidash_domain::memory::Recall::Ready { units }))
					if !units.is_empty()
			),
			"{result:?}"
		);
	}
	assert_eq!(
		attempts(&f.store, operation).await,
		[
			(0, "pending".to_owned()),
			(0, "pending".to_owned()),
			(1, "complete".to_owned()),
		]
	);
	// Attempt lineage and the original reservation survive retries and
	// successful policy settlement, so operator cost exports can join every call.
	let evidence = aidash_server::database::native::query(
		&Query::select()
			.columns(
				[
					"source_run_ids",
					"reserved_tokens",
					"reserved_cost_micros",
					"provider_usage",
				]
				.map(Alias::new),
			)
			.from(Alias::new("memory_model_attempts"))
			.and_where(Expr::col("operation_id").eq(Expr::value(operation)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(f.store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(evidence.len(), 3);
	for row in evidence {
		assert_eq!(
			row.try_get::<Option<serde_json::Value>>("source_run_ids")
				.unwrap(),
			Some(json!([run.id]))
		);
		assert_eq!(
			row.try_get::<Option<i64>>("reserved_tokens").unwrap(),
			Some(6)
		);
		assert_eq!(
			row.try_get::<Option<i64>>("reserved_cost_micros").unwrap(),
			Some(0)
		);
		// The embedding response supplies token usage, but no monetary charge.
		assert_eq!(
			row.try_get::<Option<serde_json::Value>>("provider_usage")
				.unwrap(),
			None
		);
	}
	// Assert: every charged attempt still counts against the durable budget.
	let calls: i64 = aidash_server::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("calls"))
			.from(Alias::new("memory_model_operations"))
			.and_where(Expr::col("id").eq(Expr::value(operation)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(f.store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(calls, 3);
	drop(provider);
}

/// Apply the same classification and fenced durable recovery as the worker.
async fn recover(store: &Store, run: Uuid, error: crate::Error) {
	let token = Uuid::new_v4();
	assert_eq!(store.lease_run(token, 30).await.unwrap().unwrap().id, run);
	aidash_harness::recovery::recover(
		&crate::apps::execution::repositories::RecoveryRepository {
			store: store.clone(),
		},
		token,
		crate::apps::execution::repositories::worker::classify_failure(&error),
		chrono::Utc::now(),
	)
	.await
	.unwrap();
}
