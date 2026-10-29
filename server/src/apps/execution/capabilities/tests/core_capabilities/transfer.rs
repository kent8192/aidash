use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use reinhardt::http::ViewResult;
use reinhardt::{Handler, Request, Response, ServerRouter};
use std::sync::atomic::{AtomicBool, Ordering};

#[rstest::rstest]
#[case::from_default(false)]
#[case::from_secondary(true)]
#[tokio::test]
async fn sibling_file_transfers_require_their_own_node_authority(
	#[future] capability_fixture: CoreFixture,
	#[case] reverse: bool,
	#[from(worker_control)] control: WorkerControl,
) {
	// Arrange: each Node has its own Workspace and admitted file Agent.
	let c = Box::pin(capability_fixture).await;
	let home = common::default_node(&c.f.store, "acme").await;
	let sender = admit(&c).await;
	let (status, second) = common::BrowserMapping::new(&c.f, "acme", "alice")
		.await
		.send(
			"POST",
			"/api/tenants/acme/nodes",
			json!({"label":"Sibling"}),
		)
		.await;
	assert_eq!(status, 201, "{second}");
	let name = second["name"].as_str().unwrap();
	let address = second["address"].as_str().unwrap();
	let (status, bindings) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/authorization/acme/catalog?node={}", home.node),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{bindings}");
	let approvals: Vec<_> = bindings.as_array().unwrap().iter().filter(|binding| binding["enabled"] == true).map(|binding| json!({"entry":{"id":binding["entry_id"],"version":binding["entry_version"]},"expected_revision":0,"enabled":true})).collect();
	let (status, approved) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/authorization/acme/catalog/approvals",
		json!({"node":name,"approvals":approvals}),
	)
	.await;
	assert_eq!(status, 200, "{approved}");
	let mut policy = c.policy.clone();
	policy["subjects"][aidash_domain::agent_subject(address, "research", "1.1.0")] =
		json!({"kind":"agent"});
	common::replace_policy(&c.f.store, "acme", policy.clone()).await;
	let (status, workspace) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/workspaces",
		json!({"title":"Sibling files","goal":"Receive selected files","node":name}),
	)
	.await;
	assert_eq!(status, 200, "{workspace}");
	let (status, task) = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Sibling recipient","description":"Receive selected files"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let (status, delegated) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", task["id"].as_str().unwrap()),
		json!({"node_id":address,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{delegated}");
	let receiver =
		c.f.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id.to_string() == task["id"].as_str().unwrap())
			.unwrap();
	assert_eq!(receiver.node.as_str(), name);
	assert_eq!(receiver.home_node, address);
	for run in [&sender, &receiver] {
		let (status, area) = request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/runs/{}/working-area", run.id),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{area}");
	}
	// Each local recipient lookup stays in the Node named by the UI.
	for (node, expected) in [(home.to_string(), &sender), (address.to_owned(), &receiver)] {
		let (status, page) = request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/file-recipients?node_id={node}"),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{page}");
		let items = page["items"].as_array().unwrap();
		assert!(!items.is_empty(), "{page}");
		assert!(items.iter().all(|item| item["node_id"] == node), "{page}");
		assert!(
			items
				.iter()
				.all(|item| item["thread_id"] == expected.task_id.to_string()),
			"{page}"
		);
	}
	// Ownership and file.transfer do not grant use of a denied sibling Node.
	let mut denied_node = policy.clone();
	denied_node["policies"].as_array_mut().unwrap().push(json!({
		"id":"no-sibling-use","effect":"deny","subjects":{"any":true},
		"actions":["node.use"],"resources":{"kinds":["node"],"ids":[name]},
	}));
	common::replace_policy(&c.f.store, "acme", denied_node).await;
	for (node, expected_status) in [(address.to_owned(), 403), (home.to_string(), 200)] {
		let (status, page) = request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/file-recipients?node_id={node}"),
			Value::Null,
		)
		.await;
		assert_eq!(status, expected_status, "{page}");
	}
	common::replace_policy(&c.f.store, "acme", policy.clone()).await;
	let (source, target) = if reverse {
		(&receiver, &sender)
	} else {
		(&sender, &receiver)
	};
	let target_address = format!("{}/{}", c.f.config.deployment, target.node);
	let (status, patched) = request(&c.app, &c.token, "POST", &format!("/api/runs/{}/patch", source.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"data.txt":null},"patch":"*** Begin Patch\n*** Add File: data.txt\n+sibling 東京\n*** End Patch"})).await;
	assert_eq!(status, 200, "{patched}");
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", source.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let file = &area["manifest"][0];
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"files":[{"file_id":file["file_id"],"expected_digest":file["digest"]}],"recipient":{"node_id":target_address,"agent_id":target.agent_id,"agent_version":target.agent_version,"thread_id":target.task_id}});
	let path = format!("/api/runs/{}/files/share", source.id);
	let mut denied = policy.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"no-transfer","effect":"deny","subjects":{"any":true},"actions":["file.transfer"],"resources":{"kinds":["node"]}}));
	common::replace_policy(&c.f.store, "acme", denied).await;
	// Act / Assert: sharing with a sibling must cross file.transfer admission.
	let (status, refused) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 403, "{refused}");
	common::replace_policy(&c.f.store, "acme", policy).await;
	let (status, pending) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending["status"], "pending");
	let transfer = pending["transfer_id"].as_str().unwrap();
	let WorkerControl { stop, receiver: rx } = control;
	let worker = tokio::spawn(aidash_server::capabilities::transfer::run(c.f.clone(), rx));
	let receipt = tokio::time::timeout(std::time::Duration::from_secs(30), async {
		loop {
			let (status, state) = request(
				&c.app,
				&c.token,
				"GET",
				&format!("/api/file-transfers/{transfer}"),
				Value::Null,
			)
			.await;
			assert_eq!(status, 200, "{state}");
			if state["status"] == "completed" {
				break state;
			}
			assert_ne!(state["status"], "failed", "{state}");
			tokio::time::sleep(std::time::Duration::from_secs(1)).await;
		}
	})
	.await
	.expect("sibling transfer completes");
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	let file = &receipt["receipt"]["files"][0];
	let (status, received) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", target.id),
		json!({"file_id":file["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{received}");
	assert_eq!(received["content"], "sibling 東京\n");
	c.close().await;
}

struct TransferFixture {
	a: CoreFixture,
	b: CoreFixture,
	sender: aidash_server::domain::Run,
	receiver: aidash_server::domain::Run,
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
			node.app.context.set_singleton(node.f.clone());
			let listener = tokio::net::TcpListener::bind(
				node.f.config.endpoint.strip_prefix("http://").unwrap(),
			)
			.await
			.unwrap();
			let router: Arc<dyn Handler> = node.app.native_router();
			let context = node.app.context.clone();
			// Act: replace the stopped transport at its original peer address.
			self.servers.push(tokio::spawn(async move {
				let mut connections=tokio::task::JoinSet::new();loop {tokio::select! { accepted=listener.accept()=>{let (stream,peer)=accepted.unwrap();let router=router.clone();let context=context.clone();connections.spawn(async move {let _=reinhardt::server::HttpServer::handle_connection(stream,peer,router,Some(context)).await;});}, _=connections.join_next(),if !connections.is_empty()=>{}, }}
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
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
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
	let digest = aidash_server::capabilities::objects::digest(&bytes);
	tokio::fs::write(c.a.root.join(id.simple().to_string()), &bytes)
		.await
		.unwrap();
	// A text-only recipient must be able to receive media for storage or tools.
	let file = json!({"file_id":id,"path":"large.svg","digest":digest,"size":bytes.len(),"media_type":"image/svg+xml","scope":"working","provenance":{"kind":"explicit-transport-fixture"}});
	let mut tx = c.a.f.store.pool.fixture_driver().begin().await.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_objects"))
			.columns(["id", "tenant", "area_id", "kind", "digest", "size"].map(Alias::new))
			.from_subquery(((1..=6).map(|i| Expr::cust(format!("${i}")))).fold(
				reinhardt::query::Query::select(),
				|mut select, expr| {
					select.expr(expr);
					select
				},
			))
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
			.value_expr(
				Alias::new("used_bytes"),
				Expr::col(Alias::new("used_bytes")).add(bytes.len() as i64),
			)
			.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	{
		let query_bind_1 = Uuid::parse_str(area["id"].as_str().unwrap()).unwrap();
		let query_bind_2 = json!([file]);
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_areas"))
				.value_expr(Alias::new("manifest"), Expr::value(query_bind_2.to_owned()))
				.value(Alias::new("revision"), 2)
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
	}
	.unwrap();
	tx.commit().await.unwrap();
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"files":[{"file_id":id,"expected_digest":digest}],"recipient":{"node_id":aidash_domain::nodes::NodeAddress::derived(c.b.f.config.deployment.as_str(), "acme").to_string(),"agent_id":c.receiver.agent_id,"agent_version":c.receiver.agent_version,"thread_id":c.b.task}});
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
	let data: Value = {
		let query_bind_1 = transfer;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("data"))
				.from(Alias::new("core_records"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.a.f.store.pool.fixture_driver())
		.await
	}
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
	#[from(worker_control)] control_1: WorkerControl,
) {
	use base64::Engine;
	let mut fixture = Box::pin(chunked_transfer).await;
	let c = &mut fixture.peers;
	let identity = &fixture.identity;
	let (status, prepared) = peer_request(&c.b, &c.a, "prepare", identity.clone()).await;
	assert_eq!(status, 200, "{prepared}");
	let mut wrong = identity.clone();
	wrong["input_digest"] = json!("sha256:wrong");
	assert_ne!(peer_request(&c.b, &c.a, "prepare", wrong).await.0, 200);
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
	assert_eq!(peer_request(&c.b, &c.a, "chunk", last.clone()).await.0, 200);
	assert_eq!(
		peer_request(&c.b, &c.a, "commit", identity.clone()).await.0,
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
		peer_request(&c.b, &c.a, "prepare", identity.clone())
			.await
			.0,
		200
	);
	assert_eq!(
		peer_request(&c.b, &c.a, "chunk", last).await.0,
		200,
		"lost chunk acknowledgment retries identically"
	);
	assert_eq!(peer_request(&c.b, &c.a, "chunk", chunk(0)).await.0, 200);
	let (status, receipt) = peer_request(&c.b, &c.a, "commit", identity.clone()).await;
	assert_eq!(status, 200, "{receipt}");
	assert_eq!(
		receipt["receipt"]["files"][0]["digest"],
		aidash_server::capabilities::objects::digest(&fixture.bytes)
	);
	c.restart_servers().await;
	assert_eq!(
		peer_request(&c.b, &c.a, "status", identity.clone()).await,
		(200, receipt.clone())
	);
	let WorkerControl { stop, receiver: rx } = control_1;
	// Act: reconcile the transfer after the peer restart and lost-acknowledgement sequence.
	let worker = tokio::spawn(aidash_server::capabilities::transfer::run(
		c.a.f.clone(),
		rx,
	));
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
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(two_nodes).await;
	let (_, pending) = prepare_file(&c).await;
	let transfer = Uuid::parse_str(pending["transfer_id"].as_str().unwrap()).unwrap();
	let data: Value = {
		let query_bind_1 = transfer;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("data"))
				.from(Alias::new("core_records"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.a.f.store.pool.fixture_driver())
		.await
	}
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
	.fetch_one(c.b.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let quota = Query::update()
		.table(Alias::new("core_quotas"))
		.value_expr(Alias::new("used_bytes"), Expr::cust("$1"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
				.eq(Expr::cust("'acme'")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&quota)
		.bind(c.b.f.store.capabilities.0.retained_bytes as i64)
		.execute(c.b.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		peer_request(&c.b, &c.a, "prepare", identity.clone())
			.await
			.0,
		409
	);
	sqlx::query(&quota)
		.bind(used)
		.execute(c.b.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		peer_request(&c.b, &c.a, "prepare", identity.clone())
			.await
			.0,
		200
	);
	let mut chunk = identity.clone();
	chunk["file"] = json!(0);
	chunk["offset"] = json!(0);
	chunk["data"] = json!(base64::engine::general_purpose::STANDARD.encode("corrupted 東京\n"));
	assert_eq!(peer_request(&c.b, &c.a, "chunk", chunk).await.0, 200);
	let (status, failed) = peer_request(&c.b, &c.a, "commit", identity).await;
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
async fn two_nodes(#[future(awt)] connected_nodes: ConnectedNodes) -> TransferFixture {
	let ConnectedNodes {
		a,
		b,
		servers,
		lose_commit_reply,
	} = connected_nodes;
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
pub(super) struct ConnectedNodes {
	pub(super) a: CoreFixture,
	pub(super) b: CoreFixture,
	pub(super) servers: Vec<tokio::task::JoinHandle<()>>,
	pub(super) lose_commit_reply: Arc<AtomicBool>,
}
struct CommitReplyFault {
	router: Arc<ServerRouter>,
	fault: Arc<AtomicBool>,
}
#[async_trait::async_trait]
impl Handler for CommitReplyFault {
	async fn handle(&self, request: Request) -> ViewResult<Response> {
		let commit = request.uri.path().ends_with("/files/commit");
		let result = self.router.handle(request).await?;
		if commit && result.status.is_success() && self.fault.swap(false, Ordering::SeqCst) {
			Ok(Response::new(http::StatusCode::SERVICE_UNAVAILABLE)
				.with_body("lost final acknowledgment"))
		} else {
			Ok(result)
		}
	}
}
#[rstest::fixture]
fn peer_listener() -> BoxFuture<'static, tokio::net::TcpListener> {
	// Stable peer addresses are required by the transport restart assertions.
	async { tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap() }.boxed()
}
#[rstest::fixture]
fn lost_commit_reply() -> Arc<AtomicBool> {
	Arc::new(AtomicBool::new(false))
}
#[rstest::fixture]
pub(super) fn connected_nodes(
	#[default(2)] revision: i64,
	#[default("aidash://file-source")] _source: &str,
	#[default("aidash://file-recipient")] _recipient: &str,
	#[from(capability_fixture)]
	#[with(_source)]
	a: CoreFuture,
	#[from(capability_fixture)]
	#[with(_recipient)]
	b: CoreFuture,
	#[from(peer_listeners)] _state: PeerListeners,
	#[from(lost_commit_reply)] lose_commit_reply: Arc<AtomicBool>,
) -> BoxFuture<'static, ConnectedNodes> {
	let PeerListeners {
		source_listener,
		recipient_listener,
	} = _state;
	async move {
 let mut a=a.await;let mut b=b.await;let al=source_listener.await;let bl=recipient_listener.await;
a.f.config.endpoint = format!("http://{}", al.local_addr().unwrap());
	b.f.config.endpoint = format!("http://{}", bl.local_addr().unwrap());
	a.policy["subjects"]
		[aidash_server::domain::qualified_agent(&aidash_domain::nodes::NodeAddress::derived(b.f.config.deployment.as_str(), "acme"), "research", "1.1.0")] =
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
 for node in [&a,&b] {node.app.context.set_singleton(node.f.clone());
  let mut settings=common::settings_for(&node.f.config.database_url);
  settings.deployment.id=node.f.config.deployment.to_string();settings.deployment.endpoint=node.f.config.endpoint.clone();settings.deployment.api_token=node.f.config.api_token.clone();settings.deployment.web_dir=node.f.config.web_dir.clone();settings.deployment.lease_seconds=node.f.config.lease_seconds;node.app.context.set_singleton(settings);
 }
 let a_router:Arc<dyn Handler>=a.app.native_router();
 let b_router:Arc<dyn Handler>=Arc::new(CommitReplyFault {router:b.app.native_router(),fault:lose_commit_reply.clone()});
 let mut servers=Vec::new();
 for (listener,router,context) in [(al,a_router,a.app.context.clone()),(bl,b_router,b.app.context.clone())] {
  servers.push(tokio::spawn(async move {
   let mut connections=tokio::task::JoinSet::new();loop {tokio::select! {
    accepted=listener.accept()=>{let (stream,peer)=accepted.unwrap();let router=router.clone();let context=context.clone();connections.spawn(async move {let _=reinhardt::server::HttpServer::handle_connection(stream,peer,router,Some(context)).await;});},
    _=connections.join_next(),if !connections.is_empty()=>{},
   }}
  }));
 }
 // Both servers answer, so each Deployment can fetch the other's identity.
 common::register_pair((&a.f, &a.app, "acme"), (&b.f, &b.app, "acme")).await;
	for (home, peer) in [(&a, &b), (&b, &a)] {
		let (status, credential) = async { (200_u16, common::issued_credential(&home.f.store, "acme", "alice").await) }
		.await;
		assert_eq!(status, 200, "{credential}");
		let (status, result) = common::set_peer_mapping(&home.f, "acme", "alice", json!({"node":common::default_node(&home.f.store,"acme").await.node,"source_node":common::default_node(&peer.f.store,"acme").await,"source_subject":"alice","credential_id":credential["credential"]["id"],"expected_revision":0,"enabled":true})).await;
		assert_eq!(status, 200, "{result}");
	}

 ConnectedNodes {a,b,servers,lose_commit_reply}
}.boxed()
}

/// `sender`'s default Node calls `receiver`'s file routes as the subject
/// that owns the transfer, with a token signed by `sender`'s Deployment Key.
async fn peer_request(
	receiver: &CoreFixture,
	sender: &CoreFixture,
	operation: &str,
	value: Value,
) -> (u16, Value) {
	let node = common::default_node(&receiver.f.store, "acme").await;
	common::peer_request(
		&receiver.app,
		(&sender.f, "acme"),
		&node,
		"",
		&format!("/federation/v0.1/scoped/files/{operation}"),
		value,
	)
	.await
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
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"files":[{"file_id":file["file_id"],"expected_digest":file["digest"]}],"recipient":{"node_id":aidash_domain::nodes::NodeAddress::derived(c.b.f.config.deployment.as_str(), "acme").to_string(),"agent_id":c.receiver.agent_id,"agent_version":c.receiver.agent_version,"thread_id":c.b.task}});
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
	#[from(worker_control)] control_1: WorkerControl,
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
	let WorkerControl { stop, receiver: rx } = control_1;
	// Act: reconcile only after editing the source and arming the lost-receipt fault.
	let worker = tokio::spawn(aidash_server::capabilities::transfer::run(
		c.a.f.clone(),
		rx,
	));
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
	let identity = json!({"transfer_id":pending["transfer_id"],"input_digest":aidash_server::registry::digest(&input)});
	// The request digest includes the tool identity; discover the signed-by-service
	// descriptor through durable intent metadata, never invent transfer content.
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let data: Value = {
		let query_bind_1 = Uuid::parse_str(pending["transfer_id"].as_str().unwrap()).unwrap();
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("data"))
				.from(Alias::new("core_records"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.a.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	let mut identity = identity;
	identity["input_digest"] = data["description"]["input_digest"].clone();
	let (status, result) = peer_request(&c.b, &c.a, "prepare", identity.clone()).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		peer_request(&c.b, &c.a, "commit", identity.clone()).await.0,
		409
	);
	let bytes = base64::engine::general_purpose::STANDARD.encode("immutable 東京\n");
	let mut chunk = identity.clone();
	chunk["file"] = json!(0);
	chunk["offset"] = json!(0);
	chunk["data"] = json!(bytes);
	assert_eq!(
		peer_request(&c.b, &c.a, "chunk", chunk.clone()).await.0,
		200
	);
	assert_eq!(
		peer_request(&c.b, &c.a, "chunk", chunk.clone()).await.0,
		200
	);
	chunk["data"] = json!(base64::engine::general_purpose::STANDARD.encode("corrupted 東京\n"));
	assert_eq!(peer_request(&c.b, &c.a, "chunk", chunk).await.0, 409);
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
	crate::common::replace_policy(&c.a.f.store, "acme", revoked.clone()).await;
	assert_eq!(
		peer_request(&c.b, &c.a, "commit", identity).await.0,
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

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
async fn marketplace_admit(c: &CoreFixture) -> aidash_server::domain::Run {
	let (status, package) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/marketplace/packages",
		json!({"source":{"id":"research","version":"1.1.0"},"package_id":"research","author":"Fixture","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{package}");
	let (status, installation) = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/marketplace/packages/{}/install",
			package["key"].as_str().unwrap()
		),
		json!({"digest":package["digest"],"config":{},"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 200, "{installation}");
	let id = installation["installation"]["id"].as_str().unwrap();
	let (status, revision) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/marketplace/installations/{id}"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{revision}");
	let entry: aidash_server::registry::Entry =
		serde_json::from_value(revision["entry"].clone()).unwrap();
	let auth = aidash_server::authorization::Authorization {
		pool: c.f.store.pool.clone(),
	};
	let mut snapshot = auth.snapshot("acme").await.unwrap();
	snapshot.bundle.subjects.insert(
		aidash_server::domain::qualified_agent(
			&aidash_domain::nodes::NodeAddress::derived(c.f.config.deployment.as_str(), "acme"),
			&entry.id,
			&entry.version,
		),
		serde_json::from_value(json!({"kind":"agent"})).unwrap(),
	);
	auth.replace(
		"acme",
		snapshot.revision,
		snapshot.bundle,
		"deployment_administrator",
	)
	.await
	.unwrap();
	let (status,activated)=request(&c.app,&c.token,"POST",&format!("/api/marketplace/installations/{id}/activation"),
        json!({"tenant":"acme","revision":1,"expected_activation_revision":0,"expected_catalog_revision":0,"enabled":true})).await;
	assert_eq!(status, 200, "{activated}");
	let (status, admitted) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", c.task),
		json!({"node_id":c.f.config.deployment,"agent":{"id":entry.id,"version":entry.version}}),
	)
	.await;
	assert_eq!(status, 200, "{admitted}");
	c.f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0)
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn staged_transfer_callbacks_recheck_marketplace_dependency_approvals(
	#[case] receiver_revoked: bool,

	#[future(awt)]
	#[from(connected_nodes)]
	#[with(2, "aidash://installed-source", "aidash://installed-recipient")]
	peers: ConnectedNodes,
) {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let ConnectedNodes {
		a,
		b,
		servers,
		lose_commit_reply,
	} = peers;
	let sender = marketplace_admit(&a).await;
	let receiver = marketplace_admit(&b).await;
	let auth = aidash_server::authorization::Authorization {
		pool: a.f.store.pool.clone(),
	};
	let mut snapshot = auth.snapshot("acme").await.unwrap();
	snapshot.bundle.subjects.insert(
		aidash_server::domain::qualified_agent(
			&aidash_domain::nodes::NodeAddress::derived(b.f.config.deployment.as_str(), "acme"),
			&receiver.agent_id,
			&receiver.agent_version,
		),
		serde_json::from_value(json!({"kind":"agent"})).unwrap(),
	);
	auth.replace(
		"acme",
		snapshot.revision,
		snapshot.bundle,
		"deployment_administrator",
	)
	.await
	.unwrap();
	let c = TransferFixture {
		a,
		b,
		sender,
		receiver,
		servers,
		lose_commit_reply,
	};
	let (_, pending) = prepare_file(&c).await;
	let data: Value = {
		let query_bind_1 = Uuid::parse_str(pending["transfer_id"].as_str().unwrap()).unwrap();
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("data"))
				.from(Alias::new("core_records"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.a.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	let identity = json!({"transfer_id":pending["transfer_id"],"input_digest":data["description"]["input_digest"]});
	let (status, prepared) = peer_request(&c.b, &c.a, "prepare", identity.clone()).await;
	assert_eq!(status, 200, "{prepared}");
	let revoked = if receiver_revoked { &c.b } else { &c.a };
	let auth = aidash_server::authorization::Authorization {
		pool: revoked.f.store.pool.clone(),
	};
	auth.set_catalog(
		"acme",
		&aidash_domain::nodes::NodeName::derived("acme"),
		&aidash_server::registry::EntityRef {
			id: "model".into(),
			version: "1.0.0".into(),
		},
		1,
		false,
		"deployment_administrator",
	)
	.await
	.unwrap();
	let (status, response) = if receiver_revoked {
		use base64::Engine;
		let mut chunk = identity.clone();
		chunk["file"] = json!(0);
		chunk["offset"] = json!(0);
		chunk["data"] = json!(base64::engine::general_purpose::STANDARD.encode("immutable 東京\n"));
		peer_request(&c.b, &c.a, "chunk", chunk).await
	} else {
		peer_request(&c.a, &c.b, "describe", identity).await
	};
	assert_eq!(
		status, 403,
		"staged callbacks must reject revoked pinned dependencies: {response}"
	);
	let visible: Value = {
		let query_bind_1 = c.b.task;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("manifest"))
				.from(Alias::new("core_areas"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(thread_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.b.f.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(visible, json!([]), "staged files must remain invisible");
	c.close().await;
}

struct PeerListeners {
	source_listener: BoxFuture<'static, tokio::net::TcpListener>,
	recipient_listener: BoxFuture<'static, tokio::net::TcpListener>,
}
#[rstest::fixture]
fn peer_listeners(
	#[from(peer_listener)] source_listener: BoxFuture<'static, tokio::net::TcpListener>,
	#[from(peer_listener)] recipient_listener: BoxFuture<'static, tokio::net::TcpListener>,
) -> PeerListeners {
	PeerListeners {
		source_listener,
		recipient_listener,
	}
}

#[rstest::rstest]
#[case::name(None)]
#[case::label(Some(("/label", json!("Default"))))]
#[case::default(Some(("/default", json!(true))))]
#[tokio::test]
async fn inbound_file_transfers_use_the_receiving_nodes_policy_resource(
	#[future] two_nodes: TransferFixture,
	#[case] condition: Option<(&str, Value)>,
) {
	let c = Box::pin(two_nodes).await;
	let (_, pending) = prepare_file(&c).await;
	let receiver = common::default_node(&c.b.f.store, "acme").await;
	let mut policy = c.b.policy.clone();
	policy["policies"] = json!([
		{"id":"files","effect":"allow","subjects":{"any":true},"actions":["node.use","workspace.read","file.receive","registry.read","agent.execute","tool.invoke"],"resources":{"kinds":["*"]}},
		{"id":"receiver","effect":"allow","subjects":{"any":true},"actions":["file.transfer"],"resources":{"kinds":["node"],"ids":[receiver.node]}},
	]);
	if let Some((path, value)) = condition {
		policy["policies"][1]["condition"] = json!({"op":"eq","left":{"source":"resource","path":path},"right":{"source":"literal","value":value}});
	}
	common::replace_policy(&c.b.f.store, "acme", policy.clone()).await;
	let requester = json!({"tenant":"acme","subject":"alice","cursor":null});
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _};
	let transfer = Uuid::parse_str(pending["transfer_id"].as_str().unwrap()).unwrap();
	let data: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(transfer)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.a.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let identity = json!({"transfer_id":pending["transfer_id"],"input_digest":data["description"]["input_digest"]});
	for (operation, input) in [
		("negotiate", requester.clone()),
		("recipients", requester.clone()),
		("prepare", identity.clone()),
	] {
		let (status, body) = peer_request(&c.b, &c.a, operation, input).await;
		assert_eq!(status, 200, "{operation}: {body}");
		if operation == "negotiate" {
			assert_eq!(body["node_id"], receiver.to_string());
		}
		if operation == "recipients" {
			assert!(!body["items"].as_array().unwrap().is_empty(), "{body}");
		}
	}
	for id in [
		json!(aidash_domain::nodes::NodeName::derived("other")),
		json!(receiver.to_string()),
		json!(receiver.deployment),
	] {
		policy["policies"][1]["resources"]["ids"] = json!([id]);
		common::replace_policy(&c.b.f.store, "acme", policy.clone()).await;
		for (operation, input) in [
			("negotiate", requester.clone()),
			("recipients", requester.clone()),
			("prepare", identity.clone()),
		] {
			let (status, body) = peer_request(&c.b, &c.a, operation, input).await;
			assert_eq!(status, 403, "{operation}: {body}");
		}
	}
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_recipients_use_the_sending_runs_node_and_its_registration(
	#[future] two_nodes: TransferFixture,
) {
	let c = Box::pin(two_nodes).await;
	let home = common::default_node(&c.a.f.store, "acme").await;
	let target = common::default_node(&c.b.f.store, "acme").await;
	let (status, sibling) = common::BrowserMapping::new(&c.a.f, "acme", "alice")
		.await
		.send(
			"POST",
			"/api/tenants/acme/nodes",
			json!({"label":"Sending Node"}),
		)
		.await;
	assert_eq!(status, 201, "{sibling}");
	let sibling_name = sibling["name"].as_str().unwrap();
	let sibling_address = sibling["address"].as_str().unwrap();
	let (status, catalog) = request(
		&c.a.app,
		&c.a.token,
		"GET",
		&format!("/api/authorization/acme/catalog?node={}", home.node),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{catalog}");
	let approvals:Vec<_> = catalog.as_array().unwrap().iter().filter(|binding| binding["enabled"] == true).map(|binding|json!({"entry":{"id":binding["entry_id"],"version":binding["entry_version"]},"enabled":true,"expected_revision":0})).collect();
	let (status, body) = request(
		&c.a.app,
		&c.a.token,
		"POST",
		"/api/authorization/acme/catalog/approvals",
		json!({"node":sibling_name,"approvals":approvals}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	let mut policy = c.a.policy.clone();
	policy["subjects"][aidash_domain::agent_subject(sibling_address, "research", "1.1.0")] =
		json!({"kind":"agent"});
	common::replace_policy(&c.a.f.store, "acme", policy.clone()).await;
	let (status, workspace) = request(
		&c.a.app,
		&c.a.token,
		"POST",
		"/api/workspaces",
		json!({"title":"Source","goal":"Share from sibling","node":sibling_name}),
	)
	.await;
	assert_eq!(status, 200, "{workspace}");
	let (status, task) = request(
		&c.a.app,
		&c.a.token,
		"POST",
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Share","description":"Find recipients"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let (status, admitted) = request(
		&c.a.app,
		&c.a.token,
		"POST",
		&format!("/api/tasks/{}/delegate", task["id"].as_str().unwrap()),
		json!({"node_id":sibling_address,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{admitted}");
	let run =
		c.a.f
			.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id.to_string() == task["id"].as_str().unwrap())
			.unwrap();
	let path = format!("/api/file-recipients?node_id={target}&run_id={}", run.id);
	// Only the default Node initially has a registration; it must not stand in
	// for the sending Node or advertise recipients that this Run cannot reach.
	let (status, body) = request(&c.a.app, &c.a.token, "GET", &path, Value::Null).await;
	assert_ne!(status, 200, "the sibling has no route yet: {body}");
	use common::peering;
	let registration = peering::registration(
		&target.to_string(),
		Some((&c.b.f.config.endpoint, &peering::key_id(&c.b.f))),
	);
	let (status, body) = common::BrowserMapping::new(&c.a.f, "acme", "alice")
		.await
		.send(
			"POST",
			&peering::registration_path("acme", sibling_name),
			registration,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let registration = peering::registration(
		sibling_address,
		Some((&c.a.f.config.endpoint, &peering::key_id(&c.a.f))),
	);
	let (status, body) = common::BrowserMapping::new(&c.b.f, "acme", "alice")
		.await
		.send(
			"POST",
			&peering::registration_path("acme", target.node.as_str()),
			registration,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let credential = aidash_server::authorization::Authorization {
		pool: c.b.f.store.control_pool.clone(),
	}
	.credentials("acme")
	.await
	.unwrap()
	.into_iter()
	.find(|credential| credential.subject == "alice")
	.unwrap();
	let (status, body) = common::set_peer_mapping(&c.b.f, "acme", "alice", json!({"node":target.node,"source_node":sibling_address,"source_subject":"alice","credential_id":credential.id,"expected_revision":0,"enabled":true})).await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = request(&c.a.app, &c.a.token, "GET", &path, Value::Null).await;
	assert_eq!(status, 200, "{body}");
	assert!(
		body["items"]
			.as_array()
			.unwrap()
			.iter()
			.any(|item| item["thread_id"] == c.receiver.task_id.to_string()),
		"{body}"
	);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"no-source-node","effect":"deny","subjects":{"any":true},"actions":["node.use"],"resources":{"kinds":["node"],"ids":[sibling_name]}}));
	common::replace_policy(&c.a.f.store, "acme", policy).await;
	let (status, body) = request(&c.a.app, &c.a.token, "GET", &path, Value::Null).await;
	assert_eq!(status, 403, "{body}");
	c.close().await;
}
