//! A cited File version survives later publications until its last hold goes.
use super::*;
use aidash_application::ports::capabilities::{
	cleanup::ErasureScope as _, reclamation::ReclamationRepository as _,
};
use aidash_domain::capabilities::retention::Holder;
use base64::Engine;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};

async fn access(c: &CoreFixture) -> crate::authorization::access::Access {
	let auth = crate::authorization::Authorization {
		pool: c.f.store.pool.clone(),
	};
	let crate::authorization::identity::Actor::Subject(identity) =
		auth.authenticate(&c.token).await.unwrap()
	else {
		panic!("the fixture credential is a subject")
	};
	crate::authorization::access::Access::begin(&c.f.store, &identity)
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

async fn download(c: &CoreFixture, area: Uuid, file: Uuid) -> (u16, Value) {
	request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/working-areas/{area}/files/{file}/download"),
		Value::Null,
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
async fn a_cited_html_revision_survives_later_publications_and_release_reclaims_it(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	let run = admit(&c).await;
	let (status, created) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"index.html":null,"assets/app.js":null},"patch":"*** Begin Patch\n*** Add File: index.html\n+<html>old</html>\n*** Add File: assets/app.js\n+old()\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let html = area["manifest"]
		.as_array()
		.unwrap()
		.iter()
		.find(|file| file["path"] == "index.html")
		.unwrap()["file_id"]
		.clone();
	let sibling = area["manifest"]
		.as_array()
		.unwrap()
		.iter()
		.find(|file| file["path"] == "assets/app.js")
		.unwrap()["file_id"]
		.clone();
	let html: Uuid = serde_json::from_value(html).unwrap();
	let sibling: Uuid = serde_json::from_value(sibling).unwrap();
	let message = source(&c, run.workspace_id, "cite the HTML page").await;
	let holder = Holder::Message {
		message_id: message,
	};
	let mut authority = access(&c).await;
	let retained = crate::apps::execution::capabilities::services::retention::retain(
		&c.f.store,
		&mut authority,
		area_id,
		html,
		holder.clone(),
	);
	let hold = authority.finish(retained).await.unwrap();
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
	let (status, replaced) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":preconditions,"patch":"*** Begin Patch\n*** Update File: index.html\n@@\n-<html>old</html>\n+<html>new</html>\n*** Update File: assets/app.js\n@@\n-old()\n+new()\n*** End Patch"}),
	)
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

	// Releasing the last hold lets the ordinary reclamation erase both.
	let mut authority = access(&c).await;
	let released = crate::apps::execution::capabilities::services::retention::release(
		&c.f.store,
		&mut authority,
		&holder,
	);
	assert_eq!(authority.finish(released).await.unwrap(), 1);
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
	let (status, created) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"report.txt":null},"patch":"*** Begin Patch\n*** Add File: report.txt\n+exact bytes\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
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
	let mut authority = access(&c).await;
	let retained = crate::apps::execution::capabilities::services::retention::retain(
		&c.f.store,
		&mut authority,
		area_id,
		file,
		Holder::Message {
			message_id: message,
		},
	);
	let error = authority.finish(retained).await.unwrap_err();
	assert!(
		error.to_string().starts_with("QUOTA_EXCEEDED"),
		"a refused hold explains the ceiling, not a missing version: {error}"
	);
	assert_eq!(holds(&c, "acme").await, 0);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn cleanup_releases_holds_and_expired_federation_holds_are_swept(
	#[future] capability_fixture: CoreFixture,
) {
	let c = capability_fixture.await;
	assert!(
		crate::database::classification::purged().any(|table| table == "core_retention_holds"),
		"Tenant purge deletes retention holds"
	);
	let run = admit(&c).await;
	let (status, created) = request(
		&c.app,
		&c.token,
		"POST",
		&path_for_patch(run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"page.html":null},"patch":"*** Begin Patch\n*** Add File: page.html\n+<html>page</html>\n*** End Patch"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][0]["file_id"].clone()).unwrap();
	let message = source(&c, run.workspace_id, "cite for cleanup").await;
	let mut authority = access(&c).await;
	let retained = crate::apps::execution::capabilities::services::retention::retain(
		&c.f.store,
		&mut authority,
		area_id,
		file,
		Holder::Message {
			message_id: message,
		},
	);
	authority.finish(retained).await.unwrap();
	assert_eq!(holds(&c, "acme").await, 1);
	let native: crate::apps::execution::capabilities::serializers::contracts::Area =
		serde_json::from_value(area.clone()).unwrap();
	let domain: aidash_domain::capabilities::sessions::Area = native.into();
	let repository = crate::bootstrap::cleanup_repository(&c.f.store);
	let mut scope = repository.begin().await.unwrap();
	scope.release_holds(&domain).await.unwrap();
	scope.finish(Ok(true)).await.unwrap();
	assert_eq!(
		holds(&c, "acme").await,
		0,
		"cleanup removes the Working Area's holds"
	);

	// A Federation hold keeps its File versions only until its period ends.
	let mut authority = access(&c).await;
	let retained = crate::apps::execution::capabilities::services::retention::retain(
		&c.f.store,
		&mut authority,
		area_id,
		file,
		Holder::Federation {
			peer: "peer.example".into(),
			reference_id: "delegated-run".into(),
			expires_at: chrono::Utc::now() - chrono::Duration::seconds(1),
		},
	);
	authority.finish(retained).await.unwrap();
	assert_eq!(holds(&c, "acme").await, 1);
	crate::bootstrap::reclamation_repository(&c.f.store)
		.expire_holds()
		.await
		.unwrap();
	assert_eq!(holds(&c, "acme").await, 0);
	c.close().await;
}
