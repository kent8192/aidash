//! A cited File version survives later publications until its last hold goes.
use super::*;
use aidash_domain::capabilities::retention::Holder;
use base64::Engine;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};

async fn actor(c: &CoreFixture) -> aidash_server::authorization::identity::Actor {
	aidash_server::authorization::Authorization {
		pool: c.f.store.pool.clone(),
	}
	.authenticate(&c.token)
	.await
	.unwrap()
}

async fn counts(c: &CoreFixture, sql: &str) -> i64 {
	sqlx::query_scalar(sql)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
}

async fn holds(c: &CoreFixture, tenant: &str) -> i64 {
	let query = Query::select()
		.expr(reinhardt::query::Func::count(
			Expr::col(Alias::new("id")).into(),
		))
		.from(Alias::new("core_retention_holds"))
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
		.to_string(PostgresQueryBuilder);
	counts(c, &query).await
}

async fn held_files(c: &CoreFixture, tenant: &str) -> i64 {
	let query = Query::select()
		.expr(reinhardt::query::Func::count(
			Expr::col(Alias::new("id")).into(),
		))
		.from(Alias::new("core_retention_files"))
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(tenant)))
		.to_string(PostgresQueryBuilder);
	counts(c, &query).await
}

async fn download(c: &CoreFixture, area: Uuid, file: Uuid) -> (u16, Value) {
	common::get(
		&c.app,
		&c.token,
		&format!("/api/working-areas/{area}/files/{file}/download"),
	)
	.await
}

async fn object_exists(c: &CoreFixture, file: Uuid) -> bool {
	let query = Query::select()
		.expr(reinhardt::query::Func::count(
			Expr::col(Alias::new("id")).into(),
		))
		.from(Alias::new("core_objects"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(file)))
		.to_string(PostgresQueryBuilder);
	counts(c, &query).await > 0
}

#[rstest::rstest]
#[tokio::test]
async fn repeated_rewrites_prune_unheld_revisions_and_unique_blob_usage(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let quota = Query::select()
		.column(Alias::new("used_bytes"))
		.from(Alias::new("core_quotas"))
		.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
		.to_string(PostgresQueryBuilder);
	let baseline: i64 = sqlx::query_scalar(&quota)
		.fetch_optional(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
		.unwrap_or(0);
	let mut revision = json!(1);
	let mut digest = Value::Null;
	let mut old = String::new();
	for n in 0..20 {
		let value = format!("rewrite-{n:02}");
		let patch = if n == 0 {
			format!("*** Begin Patch\n*** Add File: report.txt\n+{value}\n*** End Patch")
		} else {
			format!(
				"*** Begin Patch\n*** Update File: report.txt\n@@\n-{old}\n+{value}\n*** End Patch"
			)
		};
		let (status, result) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":revision,"preconditions":{"report.txt":digest},"patch":patch})).await;
		assert_eq!(status, 200, "{result}");
		let (status, area) = common::get(
			&c.app,
			&c.token,
			&format!("/api/runs/{}/working-area", run.id),
		)
		.await;
		assert_eq!(status, 200, "{area}");
		revision = area["revision"].clone();
		digest = area["manifest"][0]["digest"].clone();
		old = value;
	}
	let WorkerControl { stop, receiver: rx } = worker_control();
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
	loop {
		let used: i64 = sqlx::query_scalar(&quota)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
		if used == baseline + (old.len() + 1) as i64 {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"unique usage remained {used}, baseline {baseline}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
	let _ = stop.send(true);
	worker.await.unwrap().unwrap();
	c.close().await;
}

fn text(bytes: &Value) -> String {
	String::from_utf8(
		base64::engine::general_purpose::STANDARD
			.decode(bytes["data"].as_str().unwrap())
			.unwrap(),
	)
	.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn a_cited_revision_survives_until_its_last_hold_is_released(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"index.html":null,"assets/app.js":null},"patch":"*** Begin Patch\n*** Add File: index.html\n+<html>old</html>\n*** Add File: assets/app.js\n+old()\n*** End Patch"}))
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = common::get(
		&c.app,
		&c.token,
		&format!("/api/runs/{}/working-area", run.id),
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let html: Uuid = serde_json::from_value(
		area["manifest"]
			.as_array()
			.unwrap()
			.iter()
			.find(|file| file["path"] == "index.html")
			.unwrap()["file_id"]
			.clone(),
	)
	.unwrap();
	let sibling: Uuid = serde_json::from_value(
		area["manifest"]
			.as_array()
			.unwrap()
			.iter()
			.find(|file| file["path"] == "assets/app.js")
			.unwrap()["file_id"]
			.clone(),
	)
	.unwrap();
	let message = source(&c, run.workspace_id, "cite the HTML page").await;
	let holder = Holder::Message {
		message_id: message,
	};
	let hold = aidash_server::capabilities::retention::retain_for(
		&c.f,
		actor(&c).await,
		area_id,
		html,
		holder.clone(),
	)
	.await
	.unwrap();
	assert_eq!(
		hold.files.len(),
		2,
		"an HTML cite retains its whole revision: {:?}",
		hold.files
	);

	// Reclamation now runs while a later publication replaces both files.
	let WorkerControl { stop, receiver: rx } = worker_control();
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		rx,
	));
	let preconditions = json!({
		"index.html": area["manifest"].as_array().unwrap().iter()
			.find(|file| file["path"] == "index.html").unwrap()["digest"].clone(),
		"assets/app.js": area["manifest"].as_array().unwrap().iter()
			.find(|file| file["path"] == "assets/app.js").unwrap()["digest"].clone(),
	});
	let (status, replaced) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":preconditions,"patch":"*** Begin Patch\n*** Update File: index.html\n@@\n-<html>old</html>\n+<html>new</html>\n*** Update File: assets/app.js\n@@\n-old()\n+new()\n*** End Patch"}))
	.await;
	assert_eq!(status, 200, "{replaced}");
	tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
	assert!(
		object_exists(&c, html).await,
		"the held page is still stored"
	);
	assert!(
		object_exists(&c, sibling).await,
		"the held revision's sibling is still stored"
	);
	let (status, bytes) = download(&c, area_id, html).await;
	assert_eq!(status, 200, "{bytes}");
	assert_eq!(text(&bytes), "<html>old</html>\n");
	let (status, bytes) = download(&c, area_id, sibling).await;
	assert_eq!(status, 200, "{bytes}");
	assert_eq!(text(&bytes), "old()\n");

	// Releasing the final hold makes the old revision eligible for reclamation.
	assert_eq!(
		aidash_server::capabilities::retention::release_for(&c.f, actor(&c).await, &holder)
			.await
			.unwrap(),
		1
	);

	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	while object_exists(&c, html).await || object_exists(&c, sibling).await {
		assert!(
			tokio::time::Instant::now() < deadline,
			"released retained versions were not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
	let (status, _) = download(&c, area_id, html).await;
	assert_eq!(status, 404, "a released version is no longer served");
	let _ = stop.send(true);
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_over_its_storage_ceiling_cannot_retain_a_version(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"report.txt":null},"patch":"*** Begin Patch\n*** Add File: report.txt\n+exact bytes\n*** End Patch"}))
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = common::get(
		&c.app,
		&c.token,
		&format!("/api/runs/{}/working-area", run.id),
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][0]["file_id"].clone()).unwrap();
	let limit = i64::try_from(c.f.store.capabilities.0.retained_bytes).unwrap();
	let update = Query::update()
		.table(Alias::new("core_quotas"))
		.value_expr(Alias::new("used_bytes"), Expr::value(limit + 1))
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value("acme")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let message = source(&c, run.workspace_id, "cite over quota").await;
	let error = aidash_server::capabilities::retention::retain_for(
		&c.f,
		actor(&c).await,
		area_id,
		file,
		Holder::Message {
			message_id: message,
		},
	)
	.await
	.unwrap_err();
	assert!(
		error.to_string().starts_with("QUOTA_EXCEEDED"),
		"a refused hold explains the ceiling, not a missing version: {error}"
	);
	assert_eq!(holds(&c, "acme").await, 0);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn cleanup_releases_holds_and_lets_their_versions_go(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	assert!(
		aidash_server::database::classification::purged()
			.any(|table| table == "core_retention_holds"),
		"Tenant purge deletes retention holds"
	);
	let run = admit(&c).await;
	let (status, created) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"page.html":null},"patch":"*** Begin Patch\n*** Add File: page.html\n+<html>page</html>\n*** End Patch"}))
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = common::get(
		&c.app,
		&c.token,
		&format!("/api/runs/{}/working-area", run.id),
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][0]["file_id"].clone()).unwrap();
	let message = source(&c, run.workspace_id, "cite for cleanup").await;
	aidash_server::capabilities::retention::retain_for(
		&c.f,
		actor(&c).await,
		area_id,
		file,
		Holder::Message {
			message_id: message,
		},
	)
	.await
	.unwrap();
	assert_eq!(holds(&c, "acme").await, 1);

	// An irreversible cleanup erases the Area, so it must release its holds.
	let revision = area["revision"].clone();
	let (status, confirmation) = common::post(
		&c.app,
		&c.token,
		&format!("/api/working-areas/{area_id}/deletion-confirmation"),
		json!({"expected_revision":revision}),
	)
	.await;
	assert_eq!(status, 200, "{confirmation}");
	let (status, deletion) = common::post(&c.app, &c.token, &format!("/api/working-areas/{area_id}/cleanup"), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":revision,"choice":"irreversible","confirmation_id":confirmation["confirmation_id"]}))
	.await;
	assert_eq!(status, 200, "{deletion}");
	let operation: Uuid = serde_json::from_value(deletion["operation_id"].clone()).unwrap();
	let (status, reconciled) = common::post(
		&c.app,
		&c.token,
		&format!("/api/file-cleanups/{operation}/reconcile"),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{reconciled}");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	loop {
		let (_, state) =
			common::get(&c.app, &c.token, &format!("/api/file-cleanups/{operation}")).await;
		if state["state"] == "deleted" {
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{state}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	assert_eq!(
		holds(&c, "acme").await,
		0,
		"cleanup removes the Working Area's holds"
	);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_reclamation_sweep_releases_an_expired_federation_hold(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"delegated.html":null},"patch":"*** Begin Patch\n*** Add File: delegated.html\n+<html>delegated</html>\n*** End Patch"}))
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = common::get(
		&c.app,
		&c.token,
		&format!("/api/runs/{}/working-area", run.id),
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][0]["file_id"].clone()).unwrap();
	aidash_server::capabilities::retention::retain_for(
		&c.f,
		actor(&c).await,
		area_id,
		file,
		Holder::Federation {
			peer: "peer.example".into(),
			reference_id: "delegated-run".into(),
			expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
		},
	)
	.await
	.unwrap();
	assert_eq!(holds(&c, "acme").await, 1);
	let WorkerControl { stop, receiver: rx } = worker_control();
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		rx,
	));
	tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
	assert_eq!(
		holds(&c, "acme").await,
		0,
		"the sweep releases a Federation hold whose period ended"
	);
	let _ = stop.send(true);
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn citations_of_two_files_at_one_revision_share_one_hold(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"first.txt":null,"second.txt":null},"patch":"*** Begin Patch\n*** Add File: first.txt\n+first\n*** Add File: second.txt\n+second\n*** End Patch"}))
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = common::get(
		&c.app,
		&c.token,
		&format!("/api/runs/{}/working-area", run.id),
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let manifest = area["manifest"].as_array().unwrap();
	let first: Uuid = serde_json::from_value(
		manifest
			.iter()
			.find(|file| file["path"] == "first.txt")
			.unwrap()["file_id"]
			.clone(),
	)
	.unwrap();
	let second: Uuid = serde_json::from_value(
		manifest
			.iter()
			.find(|file| file["path"] == "second.txt")
			.unwrap()["file_id"]
			.clone(),
	)
	.unwrap();
	let message = source(&c, run.workspace_id, "cite two versions").await;
	let holder = Holder::Message {
		message_id: message,
	};
	aidash_server::capabilities::retention::retain_for(
		&c.f,
		actor(&c).await,
		area_id,
		first,
		holder.clone(),
	)
	.await
	.unwrap();
	aidash_server::capabilities::retention::retain_for(
		&c.f,
		actor(&c).await,
		area_id,
		second,
		holder.clone(),
	)
	.await
	.unwrap();
	assert_eq!(holds(&c, "acme").await, 1, "the revision has one hold");
	assert_eq!(held_files(&c, "acme").await, 2, "both citations are kept");
	let manifest = aidash_server::capabilities::retention::retained_manifest_for(
		&c.f,
		actor(&c).await,
		area_id,
		second,
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(manifest.len(), 2);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn concurrent_identical_citations_leave_one_hold(#[future] capability_fixture: CoreFixture) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"shared.txt":null},"patch":"*** Begin Patch\n*** Add File: shared.txt\n+shared\n*** End Patch"}))
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = common::get(
		&c.app,
		&c.token,
		&format!("/api/runs/{}/working-area", run.id),
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][0]["file_id"].clone()).unwrap();
	let message = source(&c, run.workspace_id, "cite concurrently").await;
	let holder = Holder::Message {
		message_id: message,
	};
	let (first, second) = tokio::join!(
		aidash_server::capabilities::retention::retain_for(
			&c.f,
			actor(&c).await,
			area_id,
			file,
			holder.clone(),
		),
		aidash_server::capabilities::retention::retain_for(
			&c.f,
			actor(&c).await,
			area_id,
			file,
			holder.clone(),
		),
	);
	first.unwrap();
	second.unwrap();
	assert_eq!(holds(&c, "acme").await, 1, "one hold for the holder");
	assert_eq!(held_files(&c, "acme").await, 1, "one row per File version");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn identical_files_share_one_tenant_blob_across_areas_and_survive_one_release(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let first = admit(&c).await;
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let (status, workspace) = common::post(
		&c.app,
		&c.token,
		"/api/workspaces",
		json!({"title":"Dedup","goal":"Independent files","node":node}),
	)
	.await;
	assert_eq!(status, 200, "{workspace}");
	let (status, task) = common::post(
		&c.app,
		&c.token,
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Second area","description":"Write the same content"}),
	)
	.await;
	assert_eq!(status, 200, "{task}");
	let (status, delegated) = common::post(
		&c.app,
		&c.token,
		&format!("/api/tasks/{}/delegate", task["id"].as_str().unwrap()),
		json!({"node_id":c.f.config.deployment,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{delegated}");
	let second =
		c.f.store
			.runs(&TEST_FIXTURE)
			.await
			.unwrap()
			.into_iter()
			.find(|run| run.task_id.to_string() == task["id"].as_str().unwrap())
			.unwrap();
	let mut areas = Vec::new();
	let mut used = Vec::new();
	for (run, path) in [(first.id, "one.txt"), (second.id, "two.txt")] {
		let patch =
			format!("*** Begin Patch\n*** Add File: {path}\n+identical bytes\n*** End Patch");
		let (status, body) = common::post(&c.app, &c.token, &path_for_patch(run), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{(path):null},"patch":patch})).await;
		assert_eq!(status, 200, "{body}");
		let (status, area) =
			common::get(&c.app, &c.token, &format!("/api/runs/{run}/working-area")).await;
		assert_eq!(status, 200, "{area}");
		areas.push(area);
		let quota = Query::select()
			.column(Alias::new("used_bytes"))
			.from(Alias::new("core_quotas"))
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value("acme")))
			.to_string(PostgresQueryBuilder);
		used.push(
			sqlx::query_scalar::<_, i64>(&quota)
				.fetch_one(c.f.store.pool.fixture_driver())
				.await
				.unwrap(),
		);
	}
	assert_ne!(areas[0]["id"], areas[1]["id"]);
	assert_eq!(
		areas[0]["manifest"][0]["digest"],
		areas[1]["manifest"][0]["digest"]
	);
	assert_ne!(
		areas[0]["manifest"][0]["file_id"],
		areas[1]["manifest"][0]["file_id"]
	);
	assert_eq!(
		used[0], used[1],
		"the second reference does not charge the blob twice"
	);
	let digest = areas[0]["manifest"][0]["digest"].as_str().unwrap();
	let blobs = Query::select()
		.expr(reinhardt::query::Func::count(
			Expr::col(Alias::new("digest")).into(),
		))
		.from(Alias::new("core_blobs"))
		.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value("acme")))
		.and_where(Expr::col(Alias::new("digest")).eq(Expr::value(digest)))
		.to_string(PostgresQueryBuilder);
	assert_eq!(counts(&c, &blobs).await, 1);
	let second_area: Uuid = serde_json::from_value(areas[1]["id"].clone()).unwrap();
	let second_file: Uuid =
		serde_json::from_value(areas[1]["manifest"][0]["file_id"].clone()).unwrap();
	let (status, bytes) = download(&c, second_area, second_file).await;
	assert_eq!(status, 200, "{bytes}");
	assert_eq!(text(&bytes), "identical bytes\n");
	let first_file: Uuid =
		serde_json::from_value(areas[0]["manifest"][0]["file_id"].clone()).unwrap();
	let (status, changed) = common::post(&c.app, &c.token, &path_for_patch(first.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":areas[0]["revision"],"preconditions":{"one.txt":digest},"patch":"*** Begin Patch\n*** Update File: one.txt\n@@\n-identical bytes\n+different bytes\n*** End Patch"})).await;
	assert_eq!(status, 200, "{changed}");

	let WorkerControl { stop, receiver: rx } = worker_control();
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	while object_exists(&c, first_file).await {
		assert!(
			tokio::time::Instant::now() < deadline,
			"released reference was not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
	assert_eq!(
		counts(&c, &blobs).await,
		1,
		"the second area still owns the shared blob"
	);
	let (status, bytes) = download(&c, second_area, second_file).await;
	assert_eq!(status, 200, "{bytes}");
	assert_eq!(text(&bytes), "identical bytes\n");
	let _ = stop.send(true);
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn retained_manifest_uses_the_covering_hold_revision_and_requires_a_live_hold(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let (status, created) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"index.html":null,"app.js":null},"patch":"*** Begin Patch\n*** Add File: index.html\n+<html>page</html>\n*** Add File: app.js\n+old()\n*** End Patch"})).await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = common::get(
		&c.app,
		&c.token,
		&format!("/api/runs/{}/working-area", run.id),
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let entries = area["manifest"].as_array().unwrap();
	let html: Uuid = serde_json::from_value(
		entries.iter().find(|f| f["path"] == "index.html").unwrap()["file_id"].clone(),
	)
	.unwrap();
	let original = entries.iter().find(|f| f["path"] == "app.js").unwrap();
	let original_id: Uuid = serde_json::from_value(original["file_id"].clone()).unwrap();
	let holder = Holder::Message {
		message_id: source(&c, run.workspace_id, "hold this page").await,
	};
	aidash_server::capabilities::retention::retain_for(
		&c.f,
		actor(&c).await,
		area_id,
		html,
		holder.clone(),
	)
	.await
	.unwrap();
	let (status, replaced) = common::post(&c.app, &c.token, &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"app.js":original["digest"]},"patch":"*** Begin Patch\n*** Update File: app.js\n@@\n-old()\n+new()\n*** End Patch"})).await;
	assert_eq!(status, 200, "{replaced}");
	let retained = aidash_server::capabilities::retention::retained_manifest_for(
		&c.f,
		actor(&c).await,
		area_id,
		html,
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(
		retained
			.iter()
			.find(|f| f.path == "app.js")
			.unwrap()
			.file_id,
		original_id
	);
	aidash_server::capabilities::retention::release_for(&c.f, actor(&c).await, &holder)
		.await
		.unwrap();
	assert!(
		aidash_server::capabilities::retention::retained_manifest_for(
			&c.f,
			actor(&c).await,
			area_id,
			html
		)
		.await
		.unwrap()
		.is_none()
	);
	c.close().await;
}
