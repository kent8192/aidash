use super::*;
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

struct RecoveryFixture {
	c: CoreFixture,
	area: Value,
	cleaned: Value,
	workspace: Uuid,
	thread: Value,
	new_thread: Value,
	bob: String,
}

#[rstest::fixture]
fn recovery_fixture(
	#[future] capability_fixture: CoreFixture,
) -> impl std::future::Future<Output = RecoveryFixture> {
	let capability_fixture = Box::pin(capability_fixture);
	async move {
		let c = capability_fixture.await;
		let workspace = c.f.store.task(c.task).await.unwrap().workspace_id;
		let mut threads = vec![];
		for content in ["Retain these files", "Authorized restoration destination"] {
			let root = source(&c, workspace, content).await;
			let (status, thread) = request(
				&c.app,
				&c.token,
				"POST",
				&format!("/api/workspaces/{workspace}/threads"),
				json!({"root_message_id":root}),
			)
			.await;
			assert_eq!(status, 200, "{thread}");
			threads.push(thread);
		}
		let thread = threads.remove(0);
		let new_thread = threads.remove(0);
		let (status, area) = request(&c.app, &c.token, "POST", &format!("/api/workspaces/{workspace}/threads/{}/agents/research/runs", thread["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Retained work","description":"Preserve exact bytes"})).await;
		assert_eq!(status, 200, "{area}");
		let (_, session) = request(
			&c.app,
			&c.token,
			"GET",
			&format!(
				"/api/working-areas/{}/session",
				area["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		let run = session["active_run_id"].as_str().unwrap();
		let (status, patch) = request(&c.app, &c.token, "POST", &format!("/api/runs/{run}/patch"), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"saved.txt":null},"patch":"*** Begin Patch\n*** Add File: saved.txt\n+Keep 東京 exactly\n*** End Patch"})).await;
		assert_eq!(status, 200, "{patch}");
		let (_, area) = request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/runs/{run}/working-area"),
			Value::Null,
		)
		.await;
		let (status, cleanup) = request(&c.app, &c.token, "POST", &format!("/api/working-areas/{}/cleanup", area["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"recoverable"})).await;
		assert_eq!(status, 200, "{cleanup}");
		let (status, cleaned) = request(
			&c.app,
			&c.token,
			"POST",
			&format!(
				"/api/file-cleanups/{}/reconcile",
				cleanup["operation_id"].as_str().unwrap()
			),
			json!({}),
		)
		.await;
		assert_eq!(status, 200, "{cleaned}");
		assert_eq!(cleaned["state"], "recoverable");
		let (_, credential) = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"bob"}),
		)
		.await;
		let bob = credential["token"].as_str().unwrap().to_owned();
		RecoveryFixture {
			c,
			area,
			cleaned,
			workspace,
			thread,
			new_thread,
			bob,
		}
	}
}

async fn keep(f: &RecoveryFixture, thread_delete: bool) -> Value {
	let id = f.area["id"].as_str().unwrap();
	let (path, input) = if thread_delete {
		(
			format!(
				"/api/workspaces/{}/threads/{}/delete",
				f.workspace,
				f.thread["id"].as_str().unwrap()
			),
			json!({"idempotency_key":Uuid::new_v4(),"files":[{"area_id":id,"expected_revision":f.cleaned["revision"],"choice":"keep"}]}),
		)
	} else {
		(
			format!("/api/working-areas/{id}/cleanup"),
			json!({"idempotency_key":Uuid::new_v4(),"expected_revision":f.cleaned["revision"],"choice":"keep"}),
		)
	};
	let (status, result) = request(&f.c.app, &f.c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(
		request(&f.c.app, &f.c.token, "POST", &path, input).await,
		(200, result)
	);
	let (_, inventory) = request(
		&f.c.app,
		&f.c.token,
		"GET",
		"/api/working-files",
		Value::Null,
	)
	.await;
	inventory["items"][0].clone()
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn keep_converts_the_existing_recovery_snapshot_to_retained_storage(
	#[future] recovery_fixture: RecoveryFixture,
	#[case] thread_delete: bool,
) {
	let f = Box::pin(recovery_fixture).await;
	let retained = keep(&f, thread_delete).await;
	assert_eq!(retained["state"], "retained");
	assert_eq!(retained["snapshot_id"], f.cleaned["operation_id"]);
	assert!(retained["recovery_expires_at"].is_null());
	if !thread_delete {
		let (status, deleted) = request(&f.c.app, &f.c.token, "POST", &format!("/api/workspaces/{}/threads/{}/delete", f.workspace, f.thread["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"files":[{"area_id":f.area["id"],"expected_revision":retained["revision"],"choice":"keep"}]})).await;
		assert_eq!(
			status, 200,
			"retaining a snapshot must not block later thread deletion: {deleted}"
		);
	}
	let id: Uuid = serde_json::from_value(f.cleaned["operation_id"].clone()).unwrap();
	// Even an expiry job selected before Keep must reload the retained record.
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_records"))
			.value(Alias::new("expires_at"), Expr::cust("$2"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(chrono::Utc::now() - chrono::Duration::seconds(1))
	.execute(&f.c.f.store.pool)
	.await
	.unwrap();
	let (status, reconciled) = request(
		&f.c.app,
		&f.c.token,
		"POST",
		&format!("/api/file-cleanups/{id}/reconcile"),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{reconciled}");
	assert_eq!(reconciled["state"], "kept");
	let (status, restored) = request(&f.c.app, &f.c.token, "POST", &format!("/api/working-areas/{}/restore", f.area["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":retained["revision"],"snapshot_id":id,"thread_id":f.new_thread["id"]})).await;
	assert_eq!(status, 200, "{restored}");
	assert_eq!(
		restored["manifest"][0]["digest"],
		f.area["manifest"][0]["digest"]
	);
	assert_eq!(restored["owner"], "alice");
	// A consumed retained snapshot becomes ordinary working storage. Replacing
	// it must free its object and quota without deleting the restored area.
	let (status, admitted) = request(&f.c.app, &f.c.token, "POST", &format!("/api/workspaces/{}/threads/{}/agents/research/runs", f.workspace, f.new_thread["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Continue restored work","description":"Replace the saved file"})).await;
	assert_eq!(status, 200, "{admitted}");
	let (_, session) = request(
		&f.c.app,
		&f.c.token,
		"GET",
		&format!(
			"/api/working-areas/{}/session",
			restored["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	let run = session["active_run_id"].as_str().unwrap();
	let old = &restored["manifest"][0];
	let old_id: Uuid = serde_json::from_value(old["file_id"].clone()).unwrap();
	let quota = Query::select()
		.column(Alias::new("used_bytes"))
		.from(Alias::new("core_quotas"))
		.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
		.to_string(PostgresQueryBuilder);
	let baseline: i64 = sqlx::query_scalar(&quota)
		.fetch_one(&f.c.f.store.pool)
		.await
		.unwrap();
	let (status, patched) = request(&f.c.app, &f.c.token, "POST", &format!("/api/runs/{run}/patch"), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":admitted["revision"],"preconditions":{"saved.txt":old["digest"]},"patch":"*** Begin Patch\n*** Update File: saved.txt\n@@\n-Keep 東京 exactly\n+replacement\n*** End Patch"})).await;
	assert_eq!(status, 200, "{patched}");
	let object_kind = Query::select()
		.column(Alias::new("kind"))
		.from(Alias::new("core_objects"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
		.to_string(PostgresQueryBuilder);
	let kind: String = sqlx::query_scalar(&object_kind)
		.bind(old_id)
		.fetch_one(&f.c.f.store.pool)
		.await
		.unwrap();
	assert_eq!(kind, "superseded_working");
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		f.c.f.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	loop {
		let kind: Option<String> = sqlx::query_scalar(&object_kind)
			.bind(old_id)
			.fetch_optional(&f.c.f.store.pool)
			.await
			.unwrap();
		if kind.is_none() {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"replaced retained bytes were not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	assert!(!f.c.root.join(old_id.simple().to_string()).exists());
	let used: i64 = sqlx::query_scalar(&quota)
		.fetch_one(&f.c.f.store.pool)
		.await
		.unwrap();
	assert_eq!(
		used,
		baseline - old["size"].as_i64().unwrap() + b"replacement\n".len() as i64
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	f.c.close().await;
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn restore_rechecks_current_quota_and_administrator_policy(
	#[future] recovery_fixture: RecoveryFixture,
	#[case] retained: bool,
) {
	let mut f = Box::pin(recovery_fixture).await;
	let revision = if retained {
		keep(&f, false).await["revision"].clone()
	} else {
		f.cleaned["revision"].clone()
	};
	let path = format!(
		"/api/working-areas/{}/restore",
		f.area["id"].as_str().unwrap()
	);
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":revision,"snapshot_id":f.cleaned["operation_id"],"thread_id":f.new_thread["id"]});
	let mut profile = (*f.c.f.store.capabilities.0).clone();
	profile.working_bytes = 1;
	f.c.f.store.capabilities = Runtime::new(profile.clone()).unwrap();
	f.c.app = aidash::api::router(f.c.f.clone());
	let (status, rejected) = request(&f.c.app, &f.c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 409, "{rejected}");
	assert_eq!(rejected["error"]["code"], "WORKING_QUOTA");
	profile.working_bytes = 1024;
	f.c.f.store.capabilities = Runtime::new(profile).unwrap();
	f.c.app = aidash::api::router(f.c.f.clone());
	assert_eq!(
		request(&f.c.app, &f.bob, "POST", &path, input.clone())
			.await
			.0,
		404
	);
	f.c.policy["subjects"]["bob"]["attributes"] = json!({"file_administrator":true});
	f.c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-restore","effect":"deny","subjects":{"ids":["bob"]},"actions":["file.restore"],"resources":{"kinds":["working_area"]}}));
	let (status, policy) = request(
		&f.c.app,
		&f.c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":f.c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{policy}");
	assert_eq!(
		request(&f.c.app, &f.bob, "POST", &path, input.clone())
			.await
			.0,
		403
	);
	f.c.policy["policies"].as_array_mut().unwrap().pop();
	let (status, policy) = request(
		&f.c.app,
		&f.c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":f.c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{policy}");
	let (status, restored) = request(&f.c.app, &f.bob, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{restored}");
	assert_eq!(restored["owner"], "alice");
	assert_eq!(
		restored["manifest"][0]["digest"],
		f.area["manifest"][0]["digest"]
	);
	assert_eq!(
		request(&f.c.app, &f.bob, "POST", &path, input).await,
		(200, restored)
	);
	f.c.close().await;
}

#[rstest::fixture]
fn sharing_generation_fixture(
	#[future] recovery_fixture: RecoveryFixture,
) -> impl std::future::Future<Output = (CoreFixture, String, Value, String)> {
	let recovery_fixture = Box::pin(recovery_fixture);
	async move {
		let mut f = recovery_fixture.await;
		let (status, restored) = request(&f.c.app, &f.c.token, "POST", &format!("/api/working-areas/{}/restore", f.area["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":f.cleaned["revision"],"snapshot_id":f.cleaned["operation_id"],"thread_id":f.new_thread["id"]})).await;
		assert_eq!(status, 200, "{restored}");
		let mut newer = f.c.f.registry.get("research", "1.1.0").await.unwrap();
		newer.version = "1.2.0".into();
		let (status, saved) = request(
			&f.c.app,
			&f.c.f.config.api_token,
			"POST",
			"/api/registry",
			json!(newer),
		)
		.await;
		assert_eq!(status, 200, "{saved}");
		let (status, saved) = request(
			&f.c.app,
			&f.c.f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"research","version":"1.2.0"},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "{saved}");
		f.c.policy["subjects"]
			[aidash::domain::qualified_agent(&f.c.f.config.node_id, "research", "1.2.0")] =
			json!({"kind":"agent"});
		let (status, saved) = request(
			&f.c.app,
			&f.c.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":f.c.policy}),
		)
		.await;
		assert_eq!(status, 200, "{saved}");
		let (status, area) = request(&f.c.app, &f.c.token, "POST", &format!("/api/workspaces/{}/threads/{}/agents/research/runs", f.workspace, f.new_thread["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.2.0","title":"New generation","description":"Current receiver version"})).await;
		assert_eq!(status, 200, "{area}");
		assert_eq!(area["id"], f.area["id"]);
		let sender = admit(&f.c).await;
		let (status, patch) = request(&f.c.app, &f.c.token, "POST", &path_for_patch(sender.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"share.txt":null},"patch":"*** Begin Patch\n*** Add File: share.txt\n+new copy\n*** End Patch"})).await;
		assert_eq!(status, 200, "{patch}");
		let (_, source) = request(
			&f.c.app,
			&f.c.token,
			"GET",
			&format!("/api/runs/{}/working-area", sender.id),
			Value::Null,
		)
		.await;
		let file = &source["manifest"][0];
		let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":source["revision"],"files":[{"file_id":file["file_id"],"expected_digest":file["digest"]}],"recipient":{"node_id":f.c.f.config.node_id,"agent_id":"research","agent_version":"1.1.0","thread_id":f.new_thread["id"]}});
		let path = format!("/api/runs/{}/files/share", sender.id);
		let (_, session) = request(
			&f.c.app,
			&f.c.token,
			"GET",
			&format!(
				"/api/working-areas/{}/session",
				area["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		let recipient_path = format!(
			"/api/runs/{}/working-area",
			session["active_run_id"].as_str().unwrap()
		);
		(f.c, path, input, recipient_path)
	}
}

#[rstest::rstest]
#[tokio::test]
async fn local_share_requires_the_version_admitted_in_the_current_generation(
	#[future] sharing_generation_fixture: (CoreFixture, String, Value, String),
) {
	let (c, path, mut input, recipient_path) = Box::pin(sharing_generation_fixture).await;
	let (_, before) = request(&c.app, &c.token, "GET", &recipient_path, Value::Null).await;
	let (status, denied) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 404, "{denied}");
	assert_eq!(
		request(&c.app, &c.token, "GET", &recipient_path, Value::Null).await,
		(200, before)
	);
	input["recipient"]["agent_version"] = json!("1.2.0");
	let (status, received) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{received}");
	assert_eq!(received["status"], "completed");
	let (status, after) = request(&c.app, &c.token, "GET", &recipient_path, Value::Null).await;
	assert_eq!(status, 200, "{after}");
	assert_eq!(after["manifest"].as_array().unwrap().len(), 2);
	c.close().await;
}

struct DeliveredShare {
	c: CoreFixture,
	path: String,
	input: Value,
	receipt: Value,
	sender_area: Value,
	recipient_path: String,
}

#[rstest::fixture]
fn delivered_share(
	#[future] sharing_generation_fixture: (CoreFixture, String, Value, String),
) -> impl std::future::Future<Output = DeliveredShare> {
	let fixture = Box::pin(sharing_generation_fixture);
	async move {
		let (c, path, mut input, recipient_path) = fixture.await;
		input["recipient"]["agent_version"] = json!("1.2.0");
		let (status, receipt) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
		assert_eq!(status, 200, "{receipt}");
		let (status, sender_area) = request(
			&c.app,
			&c.token,
			"GET",
			&path.replace("/files/share", "/working-area"),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{sender_area}");
		DeliveredShare {
			c,
			path,
			input,
			receipt,
			sender_area,
			recipient_path,
		}
	}
}

#[rstest::fixture]
fn disabled_share(
	#[future] delivered_share: DeliveredShare,
) -> impl std::future::Future<Output = DeliveredShare> {
	let fixture = Box::pin(delivered_share);
	async move {
		let mut f = fixture.await;
		let mut profile = (*f.c.f.store.capabilities.0).clone();
		profile.admission = false;
		f.c.f.store.capabilities = Runtime::new(profile).unwrap();
		f.c.app = aidash::api::router(f.c.f.clone());
		f
	}
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn rollback_blocks_new_local_and_remote_shares_without_losing_receipts(
	#[future] disabled_share: DeliveredShare,
	#[case] remote: bool,
) {
	let f = Box::pin(disabled_share).await;
	let (_, before) = request(&f.c.app, &f.c.token, "GET", &f.recipient_path, Value::Null).await;
	let mut input = f.input.clone();
	input["idempotency_key"] = json!(Uuid::new_v4());
	if remote {
		input["recipient"]["node_id"] = json!("aidash://remote-recipient");
	}
	let (status, rejected) = request(&f.c.app, &f.c.token, "POST", &f.path, input).await;
	assert_eq!(status, 409, "{rejected}");
	assert_eq!(rejected["error"]["code"], "CAPABILITIES_DISABLED");
	assert_eq!(
		request(&f.c.app, &f.c.token, "POST", &f.path, f.input).await,
		(200, f.receipt)
	);
	assert_eq!(
		request(&f.c.app, &f.c.token, "GET", &f.recipient_path, Value::Null).await,
		(200, before)
	);
	f.c.close().await;
}

#[rstest::fixture]
fn completed_share(
	#[future] delivered_share: DeliveredShare,
) -> impl std::future::Future<Output = (DeliveredShare, String)> {
	let fixture = Box::pin(delivered_share);
	async move {
		let f = fixture.await;
		let (status, queued) = request(&f.c.app, &f.c.token, "POST", &format!("/api/working-areas/{}/queue", f.sender_area["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Successor","description":"Only the current Run may disclose files"})).await;
		assert_eq!(status, 200, "{queued}");
		let run: Uuid = serde_json::from_value(queued["active_run_id"].clone()).unwrap();
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value(Alias::new("phase"), "COMPLETED")
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(run)
		.execute(&f.c.f.store.pool)
		.await
		.unwrap();
		let successor = queued["queue"].as_array().unwrap().last().unwrap()["run_id"]
			.as_str()
			.unwrap()
			.to_owned();
		(f, successor)
	}
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn completed_sender_cannot_disclose_successor_files_locally_or_remotely(
	#[future] completed_share: (DeliveredShare, String),
	#[case] remote: bool,
) {
	let (f, successor) = Box::pin(completed_share).await;
	let (_, before) = request(&f.c.app, &f.c.token, "GET", &f.recipient_path, Value::Null).await;
	let mut input = f.input.clone();
	input["idempotency_key"] = json!(Uuid::new_v4());
	if remote {
		input["recipient"]["node_id"] = json!("aidash://remote-recipient");
	}
	let (status, rejected) = request(&f.c.app, &f.c.token, "POST", &f.path, input.clone()).await;
	assert_eq!(status, 409, "{rejected}");
	assert_eq!(rejected["error"]["code"], "RUN_NOT_ACTIVE");
	assert_eq!(
		request(&f.c.app, &f.c.token, "POST", &f.path, f.input).await,
		(200, f.receipt)
	);
	assert_eq!(
		request(&f.c.app, &f.c.token, "GET", &f.recipient_path, Value::Null).await,
		(200, before)
	);
	if !remote {
		let (status, received) = request(
			&f.c.app,
			&f.c.token,
			"POST",
			&format!("/api/runs/{successor}/files/share"),
			input,
		)
		.await;
		assert_eq!(status, 200, "{received}");
	}
	f.c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn local_received_files_remain_bound_to_the_exact_recipient_version(
	#[future] delivered_share: DeliveredShare,
) {
	let f = Box::pin(delivered_share).await;
	let (status, area) = request(&f.c.app, &f.c.token, "GET", &f.recipient_path, Value::Null).await;
	assert_eq!(status, 200, "{area}");
	let (status, rejected) = request(&f.c.app, &f.c.token, "POST", &format!("/api/working-areas/{}/queue", area["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Other version","description":"Must not inherit a version-specific disclosure"})).await;
	assert_eq!(status, 403, "{rejected}");
	assert_eq!(
		request(&f.c.app, &f.c.token, "GET", &f.recipient_path, Value::Null).await,
		(200, area)
	);
	f.c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn occupied_restoration_destination_preserves_both_areas(
	#[future] recovery_fixture: RecoveryFixture,
) {
	let f = Box::pin(recovery_fixture).await;
	let (status, occupied) = request(&f.c.app, &f.c.token, "POST", &format!("/api/workspaces/{}/threads/{}/agents/research/runs", f.workspace, f.new_thread["id"].as_str().unwrap()), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Existing work","description":"Preserve this destination"})).await;
	assert_eq!(status, 200, "{occupied}");
	let path = format!(
		"/api/working-areas/{}/restore",
		f.area["id"].as_str().unwrap()
	);
	let mut input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":f.cleaned["revision"],"snapshot_id":f.cleaned["operation_id"],"thread_id":f.new_thread["id"]});
	let (status, rejected) = request(&f.c.app, &f.c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 409, "{rejected}");
	assert_eq!(rejected["error"]["code"], "RESTORATION_THREAD_OCCUPIED");
	let (status, existing) = request(
		&f.c.app,
		&f.c.token,
		"GET",
		&format!(
			"/api/working-areas?thread_id={}",
			f.new_thread["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{existing}");
	assert_eq!(
		existing["items"],
		json!([occupied]),
		"restoration cannot replace an existing area"
	);
	input["thread_id"] = f.thread["id"].clone();
	let (status, restored) = request(&f.c.app, &f.c.token, "POST", &path, input).await;
	assert_eq!(
		status, 200,
		"rejected restoration must preserve its snapshot and revision: {restored}"
	);
	assert_eq!(
		restored["manifest"][0]["digest"],
		f.area["manifest"][0]["digest"]
	);
	f.c.close().await;
}
