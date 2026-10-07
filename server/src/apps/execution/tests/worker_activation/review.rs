use super::*;

#[rstest::rstest]
#[tokio::test]
async fn transformed_subject_requires_operator_repair(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let node = format!("aidash://transform-{}", Uuid::new_v4());
	let settings = Settings::default();
	let broker = Broker::provision(&environment.nats_url, &node, &settings)
		.await
		.unwrap();
	let stream = broker
		.context
		.get_stream(&broker.stream_name)
		.await
		.unwrap();
	let mut config = stream.cached_info().config.clone();
	config.subject_transform = Some(async_nats::jetstream::stream::SubjectTransform {
		source: broker.subject.clone(),
		destination: "unconsumed.activation".into(),
	});
	broker.context.update_stream(config).await.unwrap();
	let provision = Broker::provision(&environment.nats_url, &node, &settings).await;
	let connect = Broker::connect(&environment.nats_url, &node, &settings, true).await;
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	assert!(matches!(provision, Err(aidash_server::Error::Invalid(_))));
	assert!(matches!(connect, Err(aidash_server::Error::Invalid(_))));
}

#[rstest::rstest]
#[tokio::test]
async fn provisioning_needs_only_node_and_broker_inputs(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let node = format!("aidash://provision-{}", Uuid::new_v4());
	let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_aidash"))
		.arg("activation-provision")
		.env_clear()
		.env("AIDASH_NODE_ID", &node)
		.env("AIDASH_ACTIVATION_NATS_URL", &environment.nats_url)
		// Unrelated application settings must not be loaded by provisioning.
		.env("AIDASH_OIDC_ISSUER", "invalid")
		.output()
		.await
		.unwrap();
	assert!(
		output.status.success(),
		"{}",
		String::from_utf8_lossy(&output.stderr)
	);
	let result: Value = serde_json::from_slice(&output.stdout).unwrap();
	let broker = Broker::connect(&environment.nats_url, &node, &Settings::default(), true)
		.await
		.unwrap();
	assert_eq!(result["stream"], broker.stream_name);
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn stream_limit_covers_maximum_envelope_and_publication_headers(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let node = format!("aidash://{}{}", Uuid::new_v4().simple(), "x".repeat(68));
	let settings = Settings::default();
	let broker = Broker::provision(&environment.nats_url, &node, &settings)
		.await
		.unwrap();
	let stream = broker
		.context
		.get_stream(&broker.stream_name)
		.await
		.unwrap();
	let mut config = stream.cached_info().config.clone();
	let envelope = aidash_server::activation::Envelope {
		version: 1,
		node_id: node.clone(),
		run_id: Uuid::new_v4(),
		activation_id: Uuid::new_v4(),
		generation: i64::MAX,
	};
	let payload = serde_json::to_vec(&envelope).unwrap();
	let message_id = format!("{}:{}", envelope.activation_id, i64::MAX);
	let maximum = payload.len() + format!("NATS/1.0\r\nNats-Msg-Id: {message_id}\r\n\r\n").len();
	for limit in [1, maximum as i32 - 1] {
		config.max_message_size = limit;
		broker.context.update_stream(config.clone()).await.unwrap();
		assert!(
			Broker::provision(&environment.nats_url, &node, &settings)
				.await
				.is_err()
		);
		assert!(
			Broker::connect(&environment.nats_url, &node, &settings, false)
				.await
				.is_err()
		);
	}
	config.max_message_size = maximum as i32;
	broker.context.update_stream(config).await.unwrap();
	let compatible = Broker::provision(&environment.nats_url, &node, &settings)
		.await
		.unwrap();
	let mut headers = async_nats::HeaderMap::new();
	headers.insert("Nats-Msg-Id", message_id);
	compatible
		.context
		.publish_with_headers(compatible.subject, headers, payload.into())
		.await
		.unwrap()
		.await
		.unwrap();
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn slow_publication_ack_does_not_hold_the_visibility_gate(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let _ = tracing_subscriber::fmt()
		.with_test_writer()
		.with_env_filter("aidash=debug")
		.try_init();
	let (mut f, url, schema) = setup(&environment).await;
	let settings = Settings {
		namespace: schema.clone(),
		..Default::default()
	};
	let broker = Broker::provision(&environment.nats_url, &f.config.node_id, &settings)
		.await
		.unwrap();
	let address = reqwest::Url::parse(&environment.nats_url).unwrap();
	let upstream = format!(
		"{}:{}",
		address.host_str().unwrap(),
		address.port().unwrap()
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	f.config.nats_url = format!("nats://{}", listener.local_addr().unwrap());
	let marker = format!("HPUB {} ", broker.subject).into_bytes();
	let (blocked, _) = tokio::sync::watch::channel(false);
	let (ack_held, held) = tokio::sync::oneshot::channel();
	let release = Arc::new(tokio::sync::Notify::new());
	let resume = release.clone();
	let proxy = tokio::spawn(async move {
		let (client, _) = listener.accept().await.unwrap();
		let server = tokio::net::TcpStream::connect(upstream).await.unwrap();
		let (mut client_read, mut client_write) = client.into_split();
		let (mut server_read, mut server_write) = server.into_split();
		let blocked_read = blocked.subscribe();
		let upstream = async move {
			let mut carry = Vec::new();
			let mut buffer = [0; 8192];
			loop {
				let count = client_read.read(&mut buffer).await?;
				if count == 0 {
					return std::io::Result::Ok(());
				}
				carry.extend_from_slice(&buffer[..count]);
				if carry.windows(marker.len()).any(|part| part == marker) {
					blocked.send_replace(true);
				}
				if carry.len() > marker.len() {
					carry.drain(..carry.len() - marker.len());
				}
				server_write.write_all(&buffer[..count]).await?;
			}
		};
		let downstream = async move {
			let mut signal = Some(ack_held);
			let mut buffer = [0; 8192];
			loop {
				let count = server_read.read(&mut buffer).await?;
				if count == 0 {
					return std::io::Result::Ok(());
				}
				if *blocked_read.borrow()
					&& let Some(signal) = signal.take()
				{
					let _ = signal.send(());
					resume.notified().await;
				}
				client_write.write_all(&buffer[..count]).await?;
			}
		};
		let _ = tokio::try_join!(upstream, downstream);
	});
	let mut insert = Query::insert();
	insert.into_table(a("runs")).columns([
		a("id"),
		a("task_id"),
		a("workspace_id"),
		a("home_node"),
		a("agent_id"),
		a("agent_version"),
		a("control"),
	]);
	insert.values_panic::<_, reinhardt::query::Value>([
		Uuid::new_v4().into(),
		Uuid::new_v4().into(),
		Uuid::new_v4().into(),
		f.config.node_id.clone().into(),
		"fixture".into(),
		"1.0.0".into(),
		"PAUSED".into(),
	]);
	sqlx::query(&insert.to_string(PostgresQueryBuilder))
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	assert!(
		count(&f, "TRUE").await > 0,
		"a Run insertion must record its scheduling obligation"
	);
	let runtime = aidash_server::activation::Runtime::new(f.clone(), settings, false);
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let task = tokio::spawn(runtime.run(stopping));
	let reached = tokio::time::timeout(Duration::from_secs(5), held).await;
	if reached.is_err() {
		let obligations: Vec<Value> = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("to_jsonb(run_activations)"))
				.from(a("run_activations"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(f.store.pool.driver())
		.await
		.unwrap();
		panic!("publication did not reach the broker: {obligations:?}");
	}
	reached.unwrap().unwrap();
	let lock = sqlx::query(
		&Query::select()
			.column(a("singleton"))
			.from(a("atomic_gate"))
			.lock(reinhardt::query::LockType::Update)
			.lock_behavior(reinhardt::query::LockBehavior::Nowait)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.control_pool.driver())
	.await;
	release.notify_one();
	stop.send_replace(true);
	task.await.unwrap().unwrap();
	proxy.abort();
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
	assert!(
		lock.is_ok(),
		"publication retained the visibility lock: {lock:?}"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn header_only_consumer_requires_operator_repair(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let node = format!("aidash://headers-{}", Uuid::new_v4());
	let settings = Settings::default();
	let broker = Broker::provision(&environment.nats_url, &node, &settings)
		.await
		.unwrap();
	let stream = broker
		.context
		.get_stream(&broker.stream_name)
		.await
		.unwrap();
	let mut config = broker
		.consumer
		.as_ref()
		.unwrap()
		.cached_info()
		.config
		.clone();
	stream.delete_consumer("workers-v1").await.unwrap();
	config.headers_only = true;
	stream.create_consumer(config).await.unwrap();
	let result = Broker::connect(&environment.nats_url, &node, &settings, true).await;
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	assert!(
		matches!(result, Err(aidash_server::Error::Invalid(_))),
		"header-only deliveries lose activation envelopes"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn completed_dependencies_release_wait_without_expiring_timer(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = common::application(f.clone()).await;
	let (_, token, dependency) = bootstrap(&f, &app, "http://localhost:1").await;
	let parent = f.store.task(dependency).await.unwrap();
	let (status, task) = request(
		&app,
		&token,
		"POST",
		&format!("/api/workspaces/{}/tasks", parent.workspace_id),
		json!({"title":"Dependent","description":"Wait"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let task: aidash_server::domain::Task = serde_json::from_value(task).unwrap();
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{}/claim", task.id),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let run = f.store.runs().await.unwrap().remove(0);
	{
		let query_bind_1 = [dependency];
		sqlx::query(
			&Query::update()
				.table(a("tasks"))
				.value_expr(
					a("dependencies"),
					Expr::cust_with_values(
						"CAST(? AS uuid[])",
						[SqlValue::Array(
							ArrayType::Uuid,
							Some(Box::new(
								query_bind_1.iter().copied().map(SqlValue::from).collect(),
							)),
						)],
					),
				)
				.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(task.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();

	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let waiting = f.store.run(run.id).await.unwrap();
	assert_eq!(waiting.phase().as_str(), "WAITING");
	// Widen the ordinary two-second timer so elapsed test time cannot mask the bug.
	sqlx::query(
		&Query::update()
			.table(a("runs"))
			.value(
				a("pending"),
				common::pending(aidash_server::domain::RunState::Waiting(Box::new(
					aidash_server::domain::WaitingState::Dependencies {
						wake_at: chrono::Utc::now() + chrono::Duration::hours(1),
						resume: Default::default(),
					},
				))),
			)
			.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	assert!(
		f.store
			.lease_run(Uuid::new_v4(), 30)
			.await
			.unwrap()
			.is_none()
	);
	sqlx::query(
		&Query::update()
			.table(a("tasks"))
			.value(a("status"), "COMPLETED")
			.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(dependency)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	assert!(
		count(
			&f,
			&format!("run_id = '{}' AND reason = 'dependency_release'", run.id)
		)
		.await > 0
	);
	let leased = f.store.lease_run(Uuid::new_v4(), 30).await.unwrap();
	cleanup(f, &url, &schema).await;
	assert_eq!(
		leased.map(|r| r.id),
		Some(run.id),
		"dependency completion must bypass its fallback timer"
	);
}

#[rstest::rstest]
#[tokio::test]
async fn terminal_delivery_drains_a_burst_without_per_run_sleep(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let workspace = Uuid::new_v4();
	let node = f.config.node_id.clone();
	let app = Router::new().route("/federation/v0.1/workspace", post(move |Json(input): Json<Value>| {
        let node = node.clone();
        async move {
            match input["operation"].as_str().unwrap() {
                "run_message_commit" => Json(json!({"committed":true})),
                "run_message_delivery" => Json(json!({"id":Uuid::new_v4(),"workspace_id":workspace,
                    "sender":"human","content":input["data"]["content"],
                    "idempotency_key":format!("{}:{}:{}",node,input["task_id"].as_str().unwrap(),input["data"]["key"].as_str().unwrap()),
                    "created_at":chrono::Utc::now()})),
                other => panic!("unexpected operation {other}"),
            }
        }
    }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
	sqlx::query(
		&Query::insert()
			.into_table(a("peers"))
			.columns(
				[
					"node_id",
					"endpoint",
					"credential_env",
					"protocol_version",
					"enabled",
				]
				.map(a),
			)
			.values_panic::<_, reinhardt::query::Value>([
				"aidash://delivery-home".into(),
				endpoint.into(),
				"AIDASH_SECRET_TEST_PEER".into(),
				"0.2".into(),
				true.into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	let mut runs = Vec::new();
	for _ in 0..24 {
		let id = super::unblock::insert_run(&f, "ACTIVE").await;
		sqlx::query(
			&Query::update()
				.table(a("runs"))
				.value(a("home_node"), "aidash://delivery-home")
				.value(a("workspace_id"), workspace)
				.value(a("phase"), "COMPLETED")
				.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
		sqlx::query(
			&Query::insert()
				.into_table(a("run_inputs"))
				.columns(["run_id", "sender", "content", "idempotency_key"].map(a))
				.values_panic::<_, reinhardt::query::Value>([
					id.into(),
					"human".into(),
					"terminal correction".into(),
					id.to_string().into(),
				])
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
		runs.push(id);
	}
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let delivery =
		tokio::spawn(async move { harness.deliver_terminal_messages_until(stopping).await });
	let drained = tokio::time::timeout(Duration::from_secs(3), async {
		loop {
			if f.store
				.pending_terminal_run_message()
				.await
				.unwrap()
				.is_none()
			{
				break;
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await;
	stop.send_replace(true);
	delivery.await.unwrap().unwrap();
	let mut delivered = 0;
	for id in runs {
		delivered += usize::from(
			f.store.run_inputs(id).await.unwrap()[0]
				.message_id
				.is_some(),
		);
	}
	server.abort();
	cleanup(f, &url, &schema).await;
	assert!(
		drained.is_ok(),
		"healthy delivery still sleeps between Runs: {delivered}/24"
	);
	assert_eq!(delivered, 24);
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use reinhardt::query::Expr;

use reinhardt::query::{ArrayType, Value as SqlValue};

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notification_claim_disposes_invalid_context_without_poisoning_healthy_work(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _serial = PROCESS_TESTS.lock().await;
	let (mut f, url, schema) = setup(&environment).await;
	let address = std::net::TcpListener::bind("127.0.0.1:0")
		.unwrap()
		.local_addr()
		.unwrap();
	f.config.endpoint = format!("http://{address}");
	let directory = evidence_directory("invalid-state", &schema);
	std::fs::create_dir_all(&directory).unwrap();
	let calls = Arc::new(AtomicUsize::new(0));
	let counted = calls.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let provider = tokio::spawn(async move {
		axum::serve(listener,Router::new().route("/v1/chat/completions",post(move || {let calls=counted.clone();async move {
		calls.fetch_add(1,Ordering::SeqCst);Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
	}}))).await.unwrap();
	});
	let (_, token, task) = bootstrap(&f, &common::application(f.clone()).await, &endpoint).await;
	let settings = Settings {
		namespace: schema.clone(),
		..Settings::default()
	};
	let broker = Broker::provision(&f.config.nats_url, &f.config.node_id, &settings)
		.await
		.unwrap();
	let mut server = Process::start(&f, &url, &schema, "server", &directory, 0, true);
	server.ready().await;
	let mut worker = Process::start(&f, &url, &schema, "worker", &directory, 1, true);
	worker.ready().await;
	let pause = directory.join("pause-consumers");
	std::fs::write(&pause, "pause").unwrap();
	tokio::time::sleep(Duration::from_millis(300)).await;
	let invalid = admit(&f, &token, task).await;
	sqlx::query(
		&Query::update()
			.table(a("runs"))
			.value(a("context"), json!([]))
			.and_where(Expr::col(a("id")).eq(Expr::value(invalid)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	let healthy = admit(&f, &token, fresh_task(&f, &token).await).await;
	std::fs::remove_file(pause).unwrap();
	complete(&f, healthy).await;
	tokio::time::timeout(Duration::from_secs(10), async {
		while f.store.inspect_run(invalid).await.unwrap().phase
			!= aidash_server::domain::RunPhase::Failed
		{
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	})
	.await
	.unwrap();
	let inspected = f.store.inspect_run(invalid).await.unwrap();
	assert!(inspected.state_error.is_some());
	assert_eq!(
		f.store.task(task).await.unwrap().status,
		aidash_server::domain::TaskStatus::Failed
	);
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	assert_eq!(count(&f, "claim_source = 'recovery'").await, 0);
	assert!(
		count(
			&f,
			&format!("run_id='{healthy}' AND claim_source='notification'")
		)
		.await > 0
	);
	let events = f.store.events(0, None, 500).await.unwrap();
	assert!(
		events
			.iter()
			.any(|e| e.kind == "run.invalid_state" && e.data["run_id"] == invalid.to_string())
	);
	worker.stop();
	server.stop();
	provider.abort();
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case::ordering("ordering")]
#[case::inactive_area("area")]
#[case::remote_dependencies("remote_dependencies")]
#[tokio::test]
async fn notification_deferral_waits_for_its_authoritative_unblock(
	#[case] blocker: &str,
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let _serial = PROCESS_TESTS.lock().await;
	let (mut f, url, schema) = setup(&environment).await;
	let address = std::net::TcpListener::bind("127.0.0.1:0")
		.unwrap()
		.local_addr()
		.unwrap();
	f.config.endpoint = format!("http://{address}");
	let directory = evidence_directory(blocker, &schema);
	std::fs::create_dir_all(&directory).unwrap();
	let calls = Arc::new(AtomicUsize::new(0));
	let counted = calls.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let provider = tokio::spawn(async move {
		axum::serve(listener, Router::new().route("/v1/chat/completions", post(move || {
			let calls = counted.clone(); async move {
				calls.fetch_add(1, Ordering::SeqCst);
				Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Done"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
			}
		}))).await.unwrap();
	});
	let app = common::application(f.clone()).await;
	let (_, token, task) = if blocker == "remote_dependencies" {
		bootstrap(&f, &app, &endpoint).await
	} else {
		// Ordering and area state constrain actual bound filesystem Tools.
		// Skill Tool availability alone does not opt in to a local area.
		common::bootstrap_with_area(&f, &app, &endpoint).await
	};
	let settings = Settings {
		namespace: schema.clone(),
		..Default::default()
	};
	let broker = Broker::provision(&f.config.nats_url, &f.config.node_id, &settings)
		.await
		.unwrap();
	let mut server = Process::start(&f, &url, &schema, "server", &directory, 0, true);
	server.ready().await;
	let run = admit(&f, &token, task).await;
	let area = if blocker == "remote_dependencies" {
		Uuid::new_v4()
	} else {
		sqlx::query_scalar::<_, Uuid>(
			&Query::select()
				.column(a("area_id"))
				.from(a("core_runs"))
				.and_where(Expr::col(a("run_id")).eq(Expr::value(run)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap()
	};
	let mut predecessor = None;
	let deadline = chrono::Utc::now() + chrono::Duration::hours(1);
	if blocker == "remote_dependencies" {
		sqlx::query(
			&Query::update()
				.table(a("runs"))
				.value(a("home_node"), "aidash://remote-home")
				.value(a("phase"), "WAITING")
				.value(
					a("pending"),
					common::pending(aidash_server::domain::RunState::Waiting(Box::new(
						aidash_server::domain::WaitingState::Dependencies {
							wake_at: deadline,
							resume: Default::default(),
						},
					))),
				)
				.and_where(Expr::col(a("id")).eq(Expr::value(run)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	} else {
		sqlx::query(
			&Query::update()
				.table(a("core_areas"))
				.value(
					a("state"),
					if blocker == "area" {
						"paused"
					} else {
						"active"
					},
				)
				.and_where(Expr::col(a("id")).eq(Expr::value(area)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
		// Reserve zero for the predecessor before inserting it below.
		sqlx::query(
			&Query::update()
				.table(a("core_runs"))
				.value(a("sequence"), 1_i64)
				.value(a("initialized"), false)
				.and_where(Expr::col(a("run_id")).eq(Expr::value(run)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
		if blocker == "ordering" {
			let previous = unblock::insert_run(&f, "PAUSED").await;
			let generation: i64 = sqlx::query_scalar(
				&Query::select()
					.column(a("generation"))
					.from(a("core_runs"))
					.and_where(Expr::col(a("run_id")).eq(Expr::value(run)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(f.store.pool.driver())
			.await
			.unwrap();
			sqlx::query(
				&Query::insert()
					.into_table(a("core_runs"))
					.columns(["run_id", "area_id", "sequence", "generation"].map(a))
					.values_panic([
						reinhardt::query::IntoValue::into_value(previous),
						reinhardt::query::IntoValue::into_value(area),
						reinhardt::query::IntoValue::into_value(0_i64),
						reinhardt::query::IntoValue::into_value(generation),
					])
					.to_string(PostgresQueryBuilder),
			)
			.execute(f.store.pool.driver())
			.await
			.unwrap();
			predecessor = Some(previous);
		}
	}
	let mut worker = Process::start(&f, &url, &schema, "worker", &directory, 1, true);
	worker.ready().await;
	let due_predicate = if blocker == "remote_dependencies" {
		"due_at > CURRENT_TIMESTAMP"
	} else {
		"due_at IS NULL"
	};
	let predicate = format!("run_id='{run}' AND state='deferred' AND {due_predicate}");
	tokio::time::timeout(Duration::from_secs(10), async {
		while count(&f, &predicate).await == 0 {
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	})
	.await
	.expect("notification must defer to a deadline or release event");
	let epoch_query = Query::select()
		.expr(Expr::cust("MAX(publication_epoch)"))
		.from(a("run_activations"))
		.and_where(Expr::col(a("run_id")).eq(Expr::value(run)))
		.to_string(PostgresQueryBuilder);
	let epoch: i64 = sqlx::query_scalar(&epoch_query)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap();
	tokio::time::sleep(Duration::from_secs(1)).await;
	assert_eq!(
		sqlx::query_scalar::<_, i64>(&epoch_query)
			.fetch_one(f.store.pool.driver())
			.await
			.unwrap(),
		epoch,
		"blocked work must not be republished immediately"
	);
	assert_eq!(calls.load(Ordering::SeqCst), 0);
	assert!(
		f.store
			.inspect_run(run)
			.await
			.unwrap()
			.lease_owner
			.is_none()
	);
	if blocker != "remote_dependencies" {
		let (table, column, id, value) = match predecessor {
			Some(id) => ("runs", "phase", id, "COMPLETED"),
			None => ("core_areas", "state", area, "active"),
		};
		sqlx::query(
			&Query::update()
				.table(a(table))
				.value_expr(a(column), value)
				.and_where(Expr::col(a("id")).eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
		complete(&f, run).await;
		assert_eq!(calls.load(Ordering::SeqCst), 1);
		let reason = if blocker == "ordering" {
			"ordering_release"
		} else {
			"area_release"
		};
		assert!(count(&f, &format!("run_id='{run}' AND reason='{reason}'")).await > 0);
		assert!(
			count(
				&f,
				&format!("run_id='{run}' AND claim_source='notification'")
			)
			.await > 0
		);
	} else {
		let due: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
			&Query::select()
				.column(a("due_at"))
				.from(a("run_activations"))
				.and_where(Expr::col(a("run_id")).eq(Expr::value(run)))
				.and_where(Expr::col(a("state")).eq(Expr::value("deferred")))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(due.timestamp_micros(), deadline.timestamp_micros());
	}
	worker.stop();
	server.stop();
	provider.abort();
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn failure_delivery_resumes_after_authority_is_restored_without_replaying_effects(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let calls = Arc::new(AtomicUsize::new(0));
	let counted = calls.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let effects = tokio::spawn(async move {
		axum::serve(
			listener,
			Router::new().fallback(move || {
				let calls = counted.clone();
				async move {
					calls.fetch_add(1, Ordering::SeqCst);
					(
						axum::http::StatusCode::INTERNAL_SERVER_ERROR,
						"unexpected provider/tool replay",
					)
				}
			}),
		)
		.await
		.unwrap();
	});
	let app = common::application(f.clone()).await;
	let (_, token, task) = bootstrap(&f, &app, &endpoint).await;
	let (status, claimed) = common::request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let run = f
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.task_id == task)
		.unwrap()
		.id;
	sqlx::query(
		&Query::update()
			.table(a("runs"))
			.value(a("context"), json!([]))
			.and_where(Expr::col(a("id")).eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	assert!(
		f.store
			.lease_run(Uuid::new_v4(), 30)
			.await
			.unwrap()
			.is_none()
	);
	assert!(
		f.store
			.inspect_run(run)
			.await
			.unwrap()
			.state
			.as_ref()
			.unwrap()
			.failure_delivery()
	);
	let operator = &f.config.api_token;
	let (status, _) = common::request(
		&app,
		operator,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false}),
	)
	.await;
	assert_eq!(status, 200);
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	harness.worker_once().await.unwrap();
	assert_eq!(
		f.store.inspect_run(run).await.unwrap().control,
		aidash_server::domain::RunControl::Paused
	);
	assert!(!f.store.task(task).await.unwrap().status.is_terminal());
	let (status, _) = common::request(
		&app,
		operator,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":2,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200);
	let (status, resumed) = common::request(
		&app,
		&token,
		"POST",
		&format!("/api/runs/{run}/control"),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 200, "{resumed}");
	assert_eq!(resumed["state"]["data"]["reason"], "failure_delivery");
	assert!(resumed["context"].is_null());
	assert!(resumed["state_error"].is_string());
	harness.worker_once().await.unwrap();
	assert_eq!(
		f.store.inspect_run(run).await.unwrap().phase,
		aidash_server::domain::RunPhase::Failed
	);
	assert_eq!(
		f.store.task(task).await.unwrap().status,
		aidash_server::domain::TaskStatus::Failed
	);
	assert_eq!(calls.load(Ordering::SeqCst), 0);
	let invocations: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(a("invocations"))
			.and_where(Expr::col(a("run_id")).eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(invocations, 0);
	effects.abort();
	cleanup(f, &url, &schema).await;
}
