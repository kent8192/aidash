use super::*;

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
	let envelope = aidash::activation::Envelope {
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
	insert.values_panic([
		Uuid::new_v4().into(),
		Uuid::new_v4().into(),
		Uuid::new_v4().into(),
		f.config.node_id.clone().into(),
		"fixture".into(),
		"1.0.0".into(),
		"PAUSED".into(),
	]);
	sqlx::query(&insert.to_string(PostgresQueryBuilder))
		.execute(&f.store.pool)
		.await
		.unwrap();
	let runtime = aidash::activation::Runtime::new(f.clone(), settings, false);
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let task = tokio::spawn(runtime.run(stopping));
	tokio::time::timeout(Duration::from_secs(5), held)
		.await
		.unwrap()
		.unwrap();
	let lock = sqlx::query(
		&Query::select()
			.column(a("singleton"))
			.from(a("atomic_gate"))
			.lock_with_behavior(
				sea_orm::sea_query::LockType::Update,
				sea_orm::sea_query::LockBehavior::Nowait,
			)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.store.control_pool)
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
