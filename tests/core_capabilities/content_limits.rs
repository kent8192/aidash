use super::*;

#[rstest::fixture]
async fn lowered_content_fixture(#[future] capability_fixture: CoreFixture) -> CoreFixture {
	let mut c = Box::pin(capability_fixture).await;
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.limits.read_bytes = 7;
	profile.limits.search_matches = 2;
	profile.limits.search_bytes = 4096;
	profile.limits.patch_bytes = 512;
	profile.limits.reference_bytes = 32;
	profile.limits.share_file_bytes = 16;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	c
}

#[rstest::rstest]
#[tokio::test]
async fn lowered_content_limits_bound_reads_search_patches_uploads_and_shares(
	#[future] lowered_content_fixture: CoreFixture,
) {
	let c = Box::pin(lowered_content_fixture).await;
	let run = admit(&c).await;
	let message = source(&c, run.workspace_id, &"東京の資料\n".repeat(8)).await;
	let base = format!("/api/runs/{}", run.id);
	let (status, created) = request(&c.app, &c.token, "POST", &format!("{base}/files/materialize"), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"path":"notes.txt","source":{"kind":"message","message_id":message}})).await;
	assert_eq!(status, 200, "{created}");
	let file = &created["file"];
	let (status, read) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{base}/files/read"),
		json!({"file_id":file["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "東京");
	assert_eq!(read["next_offset"], 6);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&format!("{base}/files/read"),
			json!({"file_id":file["file_id"],"representation":"text","max_bytes":8})
		)
		.await
		.0,
		400
	);
	let (status, search) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{base}/files/search"),
		json!({"query":"東京","mode":"literal","scope":"working"}),
	)
	.await;
	assert_eq!(status, 200, "{search}");
	assert_eq!(search["matches"].as_array().unwrap().len(), 2);
	assert!(search["next_cursor"].is_string());
	assert!(search.to_string().len() <= 4096);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&format!("{base}/files/search"),
			json!({"query":"東京","mode":"literal","scope":"working","limit":3})
		)
		.await
		.0,
		400
	);
	let (_, skills) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{base}/skills/list"),
		json!({}),
	)
	.await;
	let skill = &skills["skills"][0];
	let (status, text) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{base}/skills/read"),
		json!({"skill_id":skill["skill_id"],"digest":skill["digest"],"path":"scripts/analyze.py"}),
	)
	.await;
	assert_eq!(status, 200, "{text}");
	assert_eq!(text["content"].as_str().unwrap().len(), 7);
	assert_eq!(text["next_offset"], 7);
	let patch = format!(
		"*** Begin Patch\n*** Add File: too-big.txt\n+{}\n*** End Patch",
		"x".repeat(512)
	);
	assert_eq!(request(&c.app, &c.token, "POST", &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":created["revision"],"preconditions":{"too-big.txt":null},"patch":patch})).await.0, 400);
	let (status, upload) = request(&c.app, &c.token, "POST", "/api/references/uploads", json!({"idempotency_key":Uuid::new_v4(),"name":"too-big.txt","media_type":"text/plain","size":33,"digest":"a".repeat(64)})).await;
	assert_eq!(status, 400, "{upload}");
	assert_eq!(upload["error"]["code"], "REFERENCE_UPLOAD_LIMIT");
	let (status, share) = request(&c.app, &c.token, "POST", &format!("{base}/files/share"), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":created["revision"],"files":[{"file_id":file["file_id"],"expected_digest":file["digest"]}],"recipient":{"node_id":c.f.config.node_id,"agent_id":"research","agent_version":"1.1.0","thread_id":Uuid::new_v4()}})).await;
	assert_eq!(status, 400, "{share}");
	assert_eq!(share["error"]["code"], "SHARE_FILE_LIMIT");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("{base}/working-area"),
		Value::Null,
	)
	.await;
	assert_eq!(area["revision"], created["revision"]);
	assert_eq!(area["manifest"].as_array().unwrap().len(), 1);
	c.close().await;
}

#[rstest::fixture]
fn search_boundary_fixture(
	#[future] lowered_content_fixture: CoreFixture,
) -> impl std::future::Future<Output = (CoreFixture, Uuid)> {
	let lowered_content_fixture = Box::pin(lowered_content_fixture);
	async move {
		use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
		let c = lowered_content_fixture.await;
		let run = admit(&c).await;
		let message = source(&c, run.workspace_id, &"matched text ".repeat(60)).await;
		let (status, large) = request(&c.app, &c.token, "POST", &format!("/api/runs/{}/files/materialize", run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"path":"a".repeat(1000),"source":{"kind":"message","message_id":message}})).await;
		assert_eq!(status, 200, "{large}");
		let (_, area) = request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/runs/{}/working-area", run.id),
			Value::Null,
		)
		.await;
		// A committed binary working object is valid input, but has no UTF-8
		// representation. Keep real bytes and a matching immutable digest.
		let bytes = b"\xff\nmatched text\n";
		let id = Uuid::new_v4();
		let digest = aidash::capabilities::objects::digest(bytes);
		tokio::fs::write(c.root.join(id.simple().to_string()), bytes)
			.await
			.unwrap();
		let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
		let file = json!({"file_id":id,"path":"binary.bin","size":bytes.len(),"digest":digest,"media_type":"application/octet-stream","scope":"working","provenance":{"kind":"binary-fixture"}});
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("core_objects"))
				.columns(["id", "tenant", "area_id", "kind", "digest", "size"].map(Alias::new))
				.values_panic((1..=6).map(|i| Expr::cust(format!("${i}"))))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind("acme")
		.bind(area_id)
		.bind("working")
		.bind(&digest)
		.bind(bytes.len() as i64)
		.execute(&c.f.store.pool)
		.await
		.unwrap();
		let mut manifest = area["manifest"].clone();
		manifest.as_array_mut().unwrap().push(file);
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_areas"))
				.value(Alias::new("manifest"), Expr::cust("$2"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(area_id)
		.bind(manifest)
		.execute(&c.f.store.pool)
		.await
		.unwrap();
		(c, run.id)
	}
}

#[rstest::rstest]
#[tokio::test]
async fn oversized_search_matches_fail_explicitly_instead_of_repeating_an_empty_cursor(
	#[future] search_boundary_fixture: (CoreFixture, Uuid),
) {
	let (c, run) = Box::pin(search_boundary_fixture).await;
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{run}/files/search"),
		json!({"query":"matched","mode":"literal","scope":"working","path":"a".repeat(1000)}),
	)
	.await;
	assert_eq!(status, 400, "{result}");
	assert_eq!(result["error"]["code"], "SEARCH_RESULT_LIMIT");
	assert!(result["next_cursor"].is_null());
	c.close().await;
}

#[rstest::rstest]
#[case("literal")]
#[case("regex")]
#[tokio::test]
async fn binary_search_reports_unavailable_text_while_path_search_remains_usable(
	#[future] search_boundary_fixture: (CoreFixture, Uuid),
	#[case] mode: &str,
) {
	let (c, run) = Box::pin(search_boundary_fixture).await;
	let path = format!("/api/runs/{run}/files/search");
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"query":"matched","mode":mode,"scope":"working","path":"binary.bin"}),
	)
	.await;
	assert_eq!(status, 400, "{result}");
	assert_eq!(result["error"]["code"], "REPRESENTATION_UNAVAILABLE");
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"query":"binary","mode":"path","scope":"working","path":"binary.bin"}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result["matches"].as_array().unwrap().len(), 1);
	let (status, batch) = request(
		&c.app,
		&c.token,
		"POST",
		&path,
		json!({"query":"not-present","mode":mode,"scope":"working"}),
	)
	.await;
	assert_eq!(status, 200, "{batch}");
	assert_eq!(batch["unavailable"].as_array().unwrap().len(), 1);
	assert_eq!(
		batch["unavailable"][0]["file_id"],
		result["matches"][0]["file_id"]
	);
	assert_eq!(
		batch["unavailable"][0]["error"],
		"REPRESENTATION_UNAVAILABLE"
	);
	c.close().await;
}
