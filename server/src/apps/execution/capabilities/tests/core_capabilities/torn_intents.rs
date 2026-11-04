use super::*;

#[rstest::rstest]
#[tokio::test]
async fn torn_intent_does_not_block_uploads_or_unrelated_blob_reclamation(
	#[future] capability_fixture: CoreFixture,
	#[from(worker_control)] control: WorkerControl,
) {
	let c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let intents = c.root.join(".intents");
	tokio::fs::create_dir_all(&intents).await.unwrap();
	// A backlog makes each pre-fix sweep encounter a malformed intent before
	// it can reach unrelated reclamation.
	for _ in 0..24 {
		tokio::fs::write(intents.join(Uuid::new_v4().to_string()), b"{")
			.await
			.unwrap();
	}
	let orphan = b"unrelated orphan";
	let digest = aidash_server::capabilities::objects::digest(orphan);
	let pool = c.f.store.pool.fixture_driver();
	fixture_blob(pool, &c.root, &digest, orphan).await;
	sqlx::query("INSERT INTO core_quotas (tenant,used_bytes) VALUES ('acme',$1) ON CONFLICT (tenant) DO UPDATE SET used_bytes=core_quotas.used_bytes+$1")
        .bind(orphan.len() as i64).execute(pool).await.unwrap();
	let worker = tokio::spawn(aidash_server::capabilities::reclamation::run(
		c.f.store.clone(),
		control.receiver,
	));
	let reclaimed = tokio::time::timeout(std::time::Duration::from_secs(12), async {
		loop {
			let blob: Option<String> = sqlx::query_scalar(
				"SELECT digest FROM core_blobs WHERE tenant='acme' AND digest=$1",
			)
			.bind(&digest)
			.fetch_optional(pool)
			.await
			.unwrap();
			if blob.is_none() {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	})
	.await
	.is_ok();
	let (status, body) = request(&c.app, &c.token, "POST", &path_for_patch(run.id), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"new.txt":null},"patch":"*** Begin Patch\n*** Add File: new.txt\n+new\n*** End Patch"})).await;
	control.stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
	assert!(
		reclaimed,
		"a torn intent stalled unrelated blob reclamation"
	);
	assert_eq!(status, 200, "a torn intent stalled an upload: {body}");
}
