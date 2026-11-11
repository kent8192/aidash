//! Empty native-memory recall in a conversation with a coordinator dependency.
use super::*;
use aidash_server::{authorization::Authorization, config::Config, federation::Federation};
use std::sync::atomic::Ordering;

#[derive(Default)]
struct ModelState {
	stage: AtomicUsize,
	child: Mutex<Option<Uuid>>,
	member_calls: AtomicUsize,
	member_requests: Mutex<Vec<serde_json::Value>>,
}

#[fixture]
fn authority_state() -> Arc<ModelState> {
	Arc::new(ModelState::default())
}

#[fixture]
fn authority_router(#[from(authority_state)] state: Arc<ModelState>) -> Arc<Router> {
	Arc::new(reinhardt::test::stub::StubRouter::new()
        .route("/v1/chat/completions", http::Method::POST, reply(move |request: reinhardt::Request| {
            let body = request.json::<serde_json::Value>().unwrap();
            let state = state.clone();
            async move {
                if state.stage.load(Ordering::SeqCst) >= 2 {
                    state.member_requests.lock().unwrap().push(body);
                }
                let calls = match state.stage.load(Ordering::SeqCst) {
                    0 => vec![("create", "task_create", json!({"title":"Research","description":"Find the answer","requirements":{}}))],
                    1 => vec![("delegate", "task_delegate", json!({"task_id":state.child.lock().unwrap().unwrap(),"node_id":aidash_domain::nodes::NodeAddress::derived("aidash://native-memory", "acme").to_string(),"agent":{"id":"a","version":"1.0.0"}}))],
                    _ if state.member_calls.fetch_add(1, Ordering::SeqCst) == 0 => vec![
                        ("recall", "memory_recall", json!({"text":"evidence","time":null,"kinds":[],"max_tokens":8192})),
                        ("observe", "workspace_observe", json!({})),
                    ],
                    _ => vec![],
                };
                let message = if calls.is_empty() { json!({"role":"assistant","content":"Complete"}) } else {
                    json!({"role":"assistant","content":null,"tool_calls":calls.into_iter().map(|(id,name,args)| json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})).collect::<Vec<_>>()})
                };
                let finish = if message.get("tool_calls").is_some() { "tool_calls" } else { "stop" };
                reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":finish,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
            }
        }))
        .route("/v1/embeddings", http::Method::POST, reply(|_request: reinhardt::Request| async move {
            reinhardt::Response::ok().with_json(&json!({"model":"fixture","data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}})).unwrap()
        })).into_server_router())
}

#[rstest]
#[case::empty_dependency(false)]
#[case::lost_run_authority(true)]
#[tokio::test]
async fn conversation_member_recalls_empty_bank_after_reading_coordinator(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[from(authority_state)] state: Arc<ModelState>,
	#[from(authority_router)]
	#[with(state.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	provider: reinhardt::test::fixtures::server::TestServerGuard,
	runtime_controls: RuntimeControls,
	#[case] revoke_run: bool,
) {
	let database = database.await;
	let endpoint = format!("{}/v1", provider.url);
	let (store, registry, _) = setup_endpoint(&database, bounds, &endpoint).await;
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let coordinator = entry(
		"agent",
		"coordinator",
		json!({"model":reference("m"),"instructions":"Delegate research","memory":null}),
	);
	registry.register(&tenant, coordinator).await.unwrap();
	let node = aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme");
	let coordinator_subject = aidash_domain::qualified_agent(&node, "coordinator", "1.0.0");
	let member_subject = aidash_domain::qualified_agent(&node, "a", "1.0.0");
	let mut policy = json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},(coordinator_subject.clone()):{"kind":"agent", "node":node.node.as_str()},(member_subject.clone()):{"kind":"agent", "node":node.node.as_str()}},"policies":[
		{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}
	]});
	let authorization = Authorization {
		pool: store.pool.clone(),
	};
	authorization
		.replace(
			"acme",
			1,
			serde_json::from_value(policy.clone()).unwrap(),
			"deployment_administrator",
		)
		.await
		.unwrap();
	let actor = administrator(&store).await;
	let Actor::Subject(identity) = &actor else {
		panic!("subject fixture")
	};
	let f = Federation {
		sandbox: Default::default(),
		registry,
		config: Config {
			smtp: None,
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
			marketplace: aidash_server::config::marketplace::MarketplaceSettings {
				endpoint: None,
				connections: Vec::new(),
				poll_seconds: 60,
			},
			common_auth: None,
			common_issuers: None,
			tier_limits: Arc::new(aidash_application::ports::nodes::NoTierLimits),
			preview: None,
			dashboard_origin: "http://localhost:3000".into(),
			scratch_policy: Default::default(),
		},
		store,
		client: runtime_controls.client,
		notify: runtime_controls.notify,
	};
	let conversation = aidash_server::authorization::interaction::conversation(
		&f,
		identity,
		aidash_application::workspaces::ConversationRequest {
			title: "Conversation",
			goal: "Delegate research",
			target: &reference("coordinator"),
			target_kind: "agent",
			node: None,
			scratch_space_id: None,
		},
	)
	.await
	.unwrap();
	aidash_server::semantic::service::configure(&f.store, &actor, conversation.workspace.id, aidash_server::semantic::ConfigureIndex {
        expected_revision: 0,
        spec: aidash_server::semantic::IndexSpec {
            embedding: serde_json::from_value(json!({"provider":"openai","endpoint":endpoint,"model":"fixture","model_version":"1.0.0","dimensions":3})).unwrap(),
            vector: aidash_domain::semantic::VectorConfig { provider:"postgres".into(), endpoint:"local".into(), credential_env:None },
            enabled:true, auto_context:false, max_sources:64, max_results:4, max_result_tokens:8192, max_input_bytes:16384, max_image_bytes: 8 * 1024 * 1024, max_images: 256,
        },
    }).await.unwrap();
	let coordinator_run = f
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.agent_id == "coordinator")
		.unwrap();
	let worker = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	let child = tokio::time::timeout(std::time::Duration::from_secs(90), async {
		loop {
			worker.worker_once().await.unwrap();
			let current = f
				.store
				.run(&TEST_FIXTURE, coordinator_run.id)
				.await
				.unwrap();
			assert_eq!(
				current.control,
				aidash_domain::RunControl::Active,
				"coordinator paused before creating child: {:?}",
				current.error
			);
			let tasks = f
				.store
				.tasks(&TEST_FIXTURE, Some(conversation.workspace.id))
				.await
				.unwrap();
			if let Some(task) = tasks
				.into_iter()
				.find(|task| task.parent_id == Some(conversation.task.id))
			{
				break task;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("coordinator creates a child task");
	*state.child.lock().unwrap() = Some(child.id);
	state.stage.store(1, Ordering::SeqCst);
	let member = tokio::time::timeout(std::time::Duration::from_secs(90), async {
		loop {
			worker.worker_once().await.unwrap();
			let current = f
				.store
				.run(&TEST_FIXTURE, coordinator_run.id)
				.await
				.unwrap();
			assert_eq!(
				current.control,
				aidash_domain::RunControl::Active,
				"coordinator paused before delegating: {:?}",
				current.error
			);
			if let Some(run) = f
				.store
				.runs(&TEST_FIXTURE)
				.await
				.unwrap()
				.into_iter()
				.find(|run| run.agent_id == "a")
			{
				break run;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("coordinator delegates to the member");
	let principal: String = aidash_server::database::native::query_scalar(
		&Query::select()
			.column(("memory_participants", "principal"))
			.from(Alias::new("memory_run_bindings"))
			.inner_join(
				Alias::new("memory_participants"),
				Expr::col(("memory_run_bindings", "participant_id"))
					.equals(("memory_participants", "id")),
			)
			.and_where(Expr::col(("memory_run_bindings", "run_id")).eq(Expr::value(member.id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(f.store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(principal, "alice");
	f.store
		.control(
			&TEST_FIXTURE,
			coordinator_run.id,
			aidash_domain::RunControlAction::Pause,
		)
		.await
		.unwrap();
	state.stage.store(2, Ordering::SeqCst);
	// Let inference record the coordinator event under the original authority.
	// Then revoke access to the coordinator's private memory before the two
	// tool calls; it has never read a memory Unit.
	tokio::time::timeout(std::time::Duration::from_secs(90), async {
		loop {
			worker.worker_once().await.unwrap();
			let current = f.store.run(&TEST_FIXTURE, member.id).await.unwrap();
			assert_eq!(
				current.control,
				aidash_domain::RunControl::Active,
				"member paused before tool calls: {:?}",
				current.error
			);
			if current.phase().as_str() == "TOOL_CALL" {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("member reaches its first tool calls");
	let denial = if revoke_run {
		json!({"id":"revoke-run","effect":"deny","subjects":{"ids":[member_subject]},"actions":["run.read"],"resources":{"kinds":["run"],"ids":[member.id.to_string()]}})
	} else {
		json!({"id":"private-coordinator","effect":"deny","subjects":{"ids":[member_subject]},"actions":["memory.read"],"resources":{"kinds":["memory"]},"condition":{"op":"eq","left":{"source":"resource","path":"/created_by"},"right":{"source":"literal","value":coordinator_subject}}})
	};
	policy["policies"].as_array_mut().unwrap().push(denial);
	tokio::time::timeout(
		std::time::Duration::from_secs(60),
		authorization.replace(
			"acme",
			2,
			serde_json::from_value(policy).unwrap(),
			"deployment_administrator",
		),
	)
	.await
	.expect("policy update must pass the suspended authority boundary")
	.unwrap();
	let settled = tokio::time::timeout(std::time::Duration::from_secs(90), async {
		loop {
			worker.worker_once().await.unwrap();
			let current = f.store.run(&TEST_FIXTURE, member.id).await.unwrap();
			if current.control == aidash_domain::RunControl::Paused
				|| current.phase().as_str() == "COMPLETED"
			{
				break current;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("member settles");
	let reads: i64 = aidash_server::database::native::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("memory_run_reads"))
			.and_where(Expr::col("run_id").eq(Expr::value(member.id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(f.store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(reads, 0, "the native bank was empty");
	let coordinator_reads: i64 = aidash_server::database::native::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("authorization_run_reads"))
			.and_where(Expr::col("run_id").eq(Expr::value(member.id)))
			.and_where(Expr::col("resource_kind").eq("run"))
			.and_where(Expr::col("resource_id").eq(Expr::value(coordinator_run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(f.store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(
		coordinator_reads, 1,
		"the coordinator must be an actual dependency"
	);
	if revoke_run {
		assert_eq!(settled.control, aidash_domain::RunControl::Paused);
		assert_eq!(
			settled.error.as_deref(),
			Some("memory_recall: an authority check refused this step")
		);
		return;
	}
	assert_eq!(
		settled.control,
		aidash_domain::RunControl::Active,
		"member paused: {:?}",
		settled.error
	);
	assert_eq!(settled.phase().as_str(), "COMPLETED", "{:?}", settled.error);
	assert!(
		state.member_calls.load(Ordering::SeqCst) >= 2,
		"both calls must reach the model continuation"
	);
	let requests = state.member_requests.lock().unwrap();
	let context: serde_json::Value = serde_json::from_str(
		requests.last().unwrap()["messages"][1]["content"]
			.as_str()
			.unwrap(),
	)
	.unwrap();
	let history = context["history"].as_array().unwrap();
	let result = |id: &str| {
		&history
			.iter()
			.find(|event| event["call"]["id"] == id)
			.unwrap()["result"]
	};
	assert_eq!(result("recall")["value"]["status"], "empty");
	assert_eq!(result("observe")["view"], "workspace_observation_v1");
}
