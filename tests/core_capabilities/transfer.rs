use super::*;
use axum::{body::Body, extract::Request, middleware::Next, response::Response};
use std::sync::atomic::{AtomicBool, Ordering};

struct TransferFixture {
	a: CoreFixture,
	b: CoreFixture,
	sender: aidash::domain::Run,
	receiver: aidash::domain::Run,
	servers: Vec<tokio::task::JoinHandle<()>>,
	lose_commit_reply: Arc<AtomicBool>,
}
impl TransferFixture {
	async fn restart_servers(&mut self) {
		for server in self.servers.drain(..) {
			server.abort();
			let _ = server.await;
		}
		for node in [&mut self.a, &mut self.b] {
			node.app = aidash::api::router(node.f.clone());
			let listener = tokio::net::TcpListener::bind(
				node.f.config.endpoint.strip_prefix("http://").unwrap(),
			)
			.await
			.unwrap();
			let app = node.app.clone();
			self.servers.push(tokio::spawn(async move {
				axum::serve(listener, app).await.unwrap();
			}));
		}
	}
	async fn close(self) {
		for server in self.servers {
			server.abort();
			let _ = server.await;
		}
		self.a.close().await;
		self.b.close().await;
	}
}

struct ChunkedTransfer {
	peers: TransferFixture,
	bytes: Vec<u8>,
	input: Value,
	identity: Value,
}
#[rstest::fixture]
async fn chunked_transfer(#[future] two_nodes: TransferFixture) -> ChunkedTransfer {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(two_nodes).await;
	let (_, area) = request(
		&c.a.app,
		&c.a.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.sender.id),
		Value::Null,
	)
	.await;
	// Explicit immutable transport fixture: 4 MiB plus a final short chunk.
	// No Shell, private-reference declassification, or mocked peer transport.
	let mut bytes = vec![b'x'; 4194304];
	bytes.extend_from_slice("final 東京\n".as_bytes());
	let id = Uuid::new_v4();
	let digest = aidash::capabilities::objects::digest(&bytes);
	tokio::fs::write(c.a.root.join(id.simple().to_string()), &bytes)
		.await
		.unwrap();
	let file = json!({"file_id":id,"path":"large.txt","digest":digest,"size":bytes.len(),"media_type":"text/plain","scope":"working","provenance":{"kind":"explicit-transport-fixture"}});
	let mut tx = c.a.f.store.pool.begin().await.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_objects"))
			.columns(["id", "tenant", "area_id", "kind", "digest", "size"].map(Alias::new))
			.values_panic((1..=6).map(|i| Expr::cust(format!("${i}"))))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind("acme")
	.bind(Uuid::parse_str(area["id"].as_str().unwrap()).unwrap())
	.bind("working")
	.bind(&digest)
	.bind(bytes.len() as i64)
	.execute(&mut *tx)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_quotas"))
			.value(
				Alias::new("used_bytes"),
				Expr::col(Alias::new("used_bytes")).add(bytes.len() as i64),
			)
			.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_areas"))
			.value(Alias::new("manifest"), Expr::cust("$2"))
			.value(Alias::new("revision"), 2)
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(Uuid::parse_str(area["id"].as_str().unwrap()).unwrap())
	.bind(json!([file]))
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"files":[{"file_id":id,"expected_digest":digest}],"recipient":{"node_id":c.b.f.config.node_id,"agent_id":"research","agent_version":"1.1.0","thread_id":c.b.task}});
	let (status, pending) = request(
		&c.a.app,
		&c.a.token,
		"POST",
		&format!("/api/runs/{}/files/share", c.sender.id),
		input.clone(),
	)
	.await;
	assert_eq!(status, 200, "{pending}");
	let transfer = Uuid::parse_str(pending["transfer_id"].as_str().unwrap()).unwrap();
	let data: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(transfer)
	.fetch_one(&c.a.f.store.pool)
	.await
	.unwrap();
	ChunkedTransfer {
		peers: c,
		bytes,
		input,
		identity: json!({"transfer_id":transfer,"input_digest":data["description"]["input_digest"]}),
	}
}

#[rstest::rstest]
#[tokio::test]
async fn chunks_resume_out_of_order_after_both_servers_restart_without_duplicate_visibility(
	#[future] chunked_transfer: ChunkedTransfer,
) {
	use base64::Engine;
	let mut fixture = Box::pin(chunked_transfer).await;
	let c = &mut fixture.peers;
	let identity = &fixture.identity;
	let (status, prepared) =
		peer_request(&c.b, &c.a.f.config.node_id, "prepare", identity.clone()).await;
	assert_eq!(status, 200, "{prepared}");
	let mut wrong = identity.clone();
	wrong["input_digest"] = json!("sha256:wrong");
	assert_ne!(
		peer_request(&c.b, &c.a.f.config.node_id, "prepare", wrong)
			.await
			.0,
		200
	);
	let chunk = |offset: usize| {
		let mut value = identity.clone();
		value["file"] = json!(0);
		value["offset"] = json!(offset);
		value["data"] = json!(
			base64::engine::general_purpose::STANDARD
				.encode(&fixture.bytes[offset..fixture.bytes.len().min(offset + 4194304)])
		);
		value
	};
	let last = chunk(4194304);
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "chunk", last.clone())
			.await
			.0,
		200
	);
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "commit", identity.clone())
			.await
			.0,
		409,
		"missing first chunk cannot publish"
	);
	let (_, hidden) = request(
		&c.b.app,
		&c.b.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.receiver.id),
		Value::Null,
	)
	.await;
	assert_eq!(hidden["manifest"], json!([]));
	c.restart_servers().await;
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "prepare", identity.clone())
			.await
			.0,
		200
	);
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "chunk", last)
			.await
			.0,
		200,
		"lost chunk acknowledgment retries identically"
	);
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "chunk", chunk(0))
			.await
			.0,
		200
	);
	let (status, receipt) =
		peer_request(&c.b, &c.a.f.config.node_id, "commit", identity.clone()).await;
	assert_eq!(status, 200, "{receipt}");
	assert_eq!(
		receipt["receipt"]["files"][0]["digest"],
		aidash::capabilities::objects::digest(&fixture.bytes)
	);
	c.restart_servers().await;
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "status", identity.clone()).await,
		(200, receipt.clone())
	);
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::transfer::run(c.a.f.clone(), rx));
	let observed = until_transfer(c, &fixture.input, &["completed"]).await;
	assert_eq!(observed["receipt"], receipt["receipt"]);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	let (_, area) = request(
		&c.b.app,
		&c.b.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.receiver.id),
		Value::Null,
	)
	.await;
	assert_eq!(area["manifest"].as_array().unwrap().len(), 1);
	assert_eq!(area["revision"], 2);
	let id = Uuid::parse_str(area["manifest"][0]["file_id"].as_str().unwrap()).unwrap();
	assert_eq!(
		tokio::fs::read(c.b.root.join(id.simple().to_string()))
			.await
			.unwrap(),
		fixture.bytes
	);
	fixture.peers.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn corrupted_first_chunk_and_quota_exhaustion_never_publish_files(
	#[future] two_nodes: TransferFixture,
) {
	use base64::Engine;
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(two_nodes).await;
	let (_, pending) = prepare_file(&c).await;
	let transfer = Uuid::parse_str(pending["transfer_id"].as_str().unwrap()).unwrap();
	let data: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(transfer)
	.fetch_one(&c.a.f.store.pool)
	.await
	.unwrap();
	let identity =
		json!({"transfer_id":transfer,"input_digest":data["description"]["input_digest"]});
	let used: i64 = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("used_bytes"))
			.from(Alias::new("core_quotas"))
			.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&c.b.f.store.pool)
	.await
	.unwrap();
	let quota = Query::update()
		.table(Alias::new("core_quotas"))
		.value(Alias::new("used_bytes"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&quota)
		.bind(c.b.f.store.capabilities.0.retained_bytes as i64)
		.execute(&c.b.f.store.pool)
		.await
		.unwrap();
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "prepare", identity.clone())
			.await
			.0,
		409
	);
	sqlx::query(&quota)
		.bind(used)
		.execute(&c.b.f.store.pool)
		.await
		.unwrap();
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "prepare", identity.clone())
			.await
			.0,
		200
	);
	let mut chunk = identity.clone();
	chunk["file"] = json!(0);
	chunk["offset"] = json!(0);
	chunk["data"] = json!(base64::engine::general_purpose::STANDARD.encode("corrupted 東京\n"));
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "chunk", chunk)
			.await
			.0,
		200
	);
	let (status, failed) = peer_request(&c.b, &c.a.f.config.node_id, "commit", identity).await;
	assert_eq!(status, 409, "{failed}");
	let (_, area) = request(
		&c.b.app,
		&c.b.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.receiver.id),
		Value::Null,
	)
	.await;
	assert_eq!(
		area["manifest"],
		json!([]),
		"corrupted bytes cannot become visible"
	);
	c.close().await;
}
#[rstest::fixture]
async fn two_nodes(#[future] test_environment: Arc<TestEnvironment>) -> TransferFixture {
	let env = test_environment.await;
	let mut a = build_core_fixture(env.clone(), "aidash://file-source").await;
	let mut b = build_core_fixture(env, "aidash://file-recipient").await;
	let (servers, lose_commit_reply) = connect_nodes(&mut a, &mut b, 2).await;
	let sender = admit(&a).await;
	let receiver = admit(&b).await;
	TransferFixture {
		a,
		b,
		sender,
		receiver,
		servers,
		lose_commit_reply,
	}
}
pub(super) async fn connect_nodes(
	a: &mut CoreFixture,
	b: &mut CoreFixture,
	revision: i64,
) -> (Vec<tokio::task::JoinHandle<()>>, Arc<AtomicBool>) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let al = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let bl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	a.f.config.endpoint = format!("http://{}", al.local_addr().unwrap());
	b.f.config.endpoint = format!("http://{}", bl.local_addr().unwrap());
	a.policy["subjects"]
		[aidash::domain::qualified_agent(&b.f.config.node_id, "research", "1.1.0")] =
		json!({"kind":"agent"});
	let (status, result) = request(
		&a.app,
		&a.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":revision,"bundle":a.policy}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	for (home, peer) in [(&*a, &*b), (&*b, &*a)] {
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("peers"))
				.columns(
					[
						"node_id",
						"endpoint",
						"credential_env",
						"protocol_version",
						"enabled",
					]
					.map(Alias::new),
				)
				.values_panic([
					Expr::cust("$1"),
					Expr::cust("$2"),
					Expr::val("AIDASH_SECRET_TEST_PEER").into(),
					Expr::val("0.1").into(),
					Expr::val(true).into(),
				])
				.to_string(PostgresQueryBuilder),
		)
		.bind(&peer.f.config.node_id)
		.bind(&peer.f.config.endpoint)
		.execute(&home.f.store.pool)
		.await
		.unwrap();
		let (status, credential) = request(
			&home.app,
			&home.f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"alice"}),
		)
		.await;
		assert_eq!(status, 200, "{credential}");
		let (status, result) = request(&home.app, &home.f.config.api_token, "POST", "/api/authorization/acme/peer-mappings", json!({"source_node":peer.f.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":credential["credential"]["id"],"expected_revision":0,"enabled":true})).await;
		assert_eq!(status, 200, "{result}");
	}
	a.app = aidash::api::router(a.f.clone());
	b.app = aidash::api::router(b.f.clone());
	let lose_commit_reply = Arc::new(AtomicBool::new(false));
	let fault = lose_commit_reply.clone();
	let bapp = b.app.clone().layer(axum::middleware::from_fn(
		move |request: Request, next: Next| {
			let fault = fault.clone();
			async move {
				let commit = request.uri().path().ends_with("/files/commit");
				let result = next.run(request).await;
				if commit && result.status().is_success() && fault.swap(false, Ordering::SeqCst) {
					Response::builder()
						.status(503)
						.body(Body::from("lost final acknowledgment"))
						.unwrap()
				} else {
					result
				}
			}
		},
	));
	let aapp = a.app.clone();
	let servers = vec![
		tokio::spawn(async move {
			axum::serve(al, aapp).await.unwrap();
		}),
		tokio::spawn(async move {
			axum::serve(bl, bapp).await.unwrap();
		}),
	];
	(servers, lose_commit_reply)
}
async fn peer_request(c: &CoreFixture, node: &str, operation: &str, value: Value) -> (u16, Value) {
	use tower::ServiceExt;
	let response = c
		.app
		.clone()
		.oneshot(
			axum::http::Request::builder()
				.method("POST")
				.uri(format!("/federation/v0.1/scoped/files/{operation}"))
				.header(
					"authorization",
					format!(
						"Bearer {}",
						std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()
					),
				)
				.header("x-aidash-node", node)
				.header("x-aidash-protocol", "0.1")
				.header("content-type", "application/json")
				.body(Body::from(value.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
		.await
		.unwrap();
	(
		status,
		serde_json::from_slice(&bytes).unwrap_or(Value::Null),
	)
}
async fn prepare_file(c: &TransferFixture) -> (Value, Value) {
	let (status, value) = request(&c.a.app, &c.a.token, "POST", &format!("/api/runs/{}/patch", c.sender.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"data.txt":null},"patch":"*** Begin Patch\n*** Add File: data.txt\n+immutable 東京\n*** End Patch"})).await;
	assert_eq!(status, 200, "{value}");
	let (_, area) = request(
		&c.a.app,
		&c.a.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.sender.id),
		Value::Null,
	)
	.await;
	let file = area["manifest"][0].clone();
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"files":[{"file_id":file["file_id"],"expected_digest":file["digest"]}],"recipient":{"node_id":c.b.f.config.node_id,"agent_id":"research","agent_version":"1.1.0","thread_id":c.b.task}});
	let (status, pending) = request(
		&c.a.app,
		&c.a.token,
		"POST",
		&format!("/api/runs/{}/files/share", c.sender.id),
		input.clone(),
	)
	.await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending["status"], "pending");
	(input, pending)
}
async fn until_transfer(c: &TransferFixture, input: &Value, states: &[&str]) -> Value {
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
	loop {
		let (status, result) = request(
			&c.a.app,
			&c.a.token,
			"POST",
			&format!("/api/runs/{}/files/share", c.sender.id),
			input.clone(),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		if states.contains(&result["status"].as_str().unwrap()) {
			return result;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"transfer not at {states:?}: {result}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
}
#[rstest::rstest]
#[tokio::test]
async fn remote_snapshot_survives_source_edit_and_lost_final_receipt(
	#[future] two_nodes: TransferFixture,
) {
	let c = Box::pin(two_nodes).await;
	let (input, pending) = prepare_file(&c).await;
	let (_, source) = request(
		&c.a.app,
		&c.a.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.sender.id),
		Value::Null,
	)
	.await;
	let (status, result) = request(&c.a.app, &c.a.token, "POST", &format!("/api/runs/{}/patch", c.sender.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"preconditions":{"data.txt":source["manifest"][0]["digest"]},"patch":"*** Begin Patch\n*** Update File: data.txt\n@@\n-immutable 東京\n+changed later\n*** End Patch"})).await;
	assert_eq!(status, 200, "{result}");
	c.lose_commit_reply.store(true, Ordering::SeqCst);
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::transfer::run(c.a.f.clone(), rx));
	let receipt = until_transfer(&c, &input, &["completed"]).await;
	assert!(
		!c.lose_commit_reply.load(Ordering::SeqCst),
		"fault must run after a real committed receiver transaction"
	);
	assert_eq!(receipt["transfer_id"], pending["transfer_id"]);
	let (_, area) = request(
		&c.b.app,
		&c.b.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.receiver.id),
		Value::Null,
	)
	.await;
	assert_eq!(
		area["manifest"].as_array().unwrap().len(),
		1,
		"a retry must not publish twice"
	);
	assert_eq!(area["revision"], 2);
	let file = &area["manifest"][0];
	assert_eq!(file["digest"], source["manifest"][0]["digest"]);
	let (status, content) = request(
		&c.b.app,
		&c.b.token,
		"POST",
		&format!("/api/runs/{}/files/read", c.receiver.id),
		json!({"file_id":file["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{content}");
	assert_eq!(content["content"], "immutable 東京\n");
	assert_eq!(
		until_transfer(&c, &input, &["completed"]).await["receipt"],
		receipt["receipt"]
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn transfer_commit_rechecks_source_and_receiver_and_keeps_chunks_invisible(
	#[future] two_nodes: TransferFixture,
) {
	use base64::Engine;
	let c = Box::pin(two_nodes).await;
	let (input, pending) = prepare_file(&c).await;
	let identity = json!({"transfer_id":pending["transfer_id"],"input_digest":aidash::registry::digest(&input)});
	// The request digest includes the tool identity; discover the signed-by-service
	// descriptor through durable intent metadata, never invent transfer content.
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let data: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(Uuid::parse_str(pending["transfer_id"].as_str().unwrap()).unwrap())
	.fetch_one(&c.a.f.store.pool)
	.await
	.unwrap();
	let mut identity = identity;
	identity["input_digest"] = data["description"]["input_digest"].clone();
	let (status, result) =
		peer_request(&c.b, &c.a.f.config.node_id, "prepare", identity.clone()).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "commit", identity.clone())
			.await
			.0,
		409
	);
	let bytes = base64::engine::general_purpose::STANDARD.encode("immutable 東京\n");
	let mut chunk = identity.clone();
	chunk["file"] = json!(0);
	chunk["offset"] = json!(0);
	chunk["data"] = json!(bytes);
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "chunk", chunk.clone())
			.await
			.0,
		200
	);
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "chunk", chunk.clone())
			.await
			.0,
		200
	);
	chunk["data"] = json!(base64::engine::general_purpose::STANDARD.encode("corrupted 東京\n"));
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "chunk", chunk)
			.await
			.0,
		409
	);
	let (_, area) = request(
		&c.b.app,
		&c.b.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.receiver.id),
		Value::Null,
	)
	.await;
	assert_eq!(area["manifest"], json!([]), "staging is not visible");
	let mut revoked = c.a.policy.clone();
	revoked["policies"].as_array_mut().unwrap().push(json!({"id":"withdraw-transfer","effect":"deny","subjects":{"ids":["alice"]},"actions":["file.share"],"resources":{"kinds":["working_area"]}}));
	let (status, result) = request(
		&c.a.app,
		&c.a.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":revoked}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		peer_request(&c.b, &c.a.f.config.node_id, "commit", identity)
			.await
			.0,
		403,
		"a peer credential cannot bypass original source revocation"
	);
	let (_, area) = request(
		&c.b.app,
		&c.b.token,
		"GET",
		&format!("/api/runs/{}/working-area", c.receiver.id),
		Value::Null,
	)
	.await;
	assert_eq!(area["manifest"], json!([]));
	c.close().await;
}
