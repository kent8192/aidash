use super::*;

#[rstest::rstest]
#[tokio::test]
async fn unrelated_upload_cannot_consume_reserved_transfer_capacity(
	#[future] capability_fixture: CoreFixture,
) {
	let mut c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let used: i64 = sqlx::query_scalar("SELECT used_bytes FROM core_quotas WHERE tenant='acme'")
		.fetch_optional(c.f.store.pool.fixture_driver())
		.await
		.unwrap()
		.unwrap_or(0);
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.retained_bytes = used as u64 + 20;
	profile.working_bytes = profile.retained_bytes;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app.context.set_singleton(c.f.clone());
	sqlx::query("INSERT INTO core_quotas (tenant,reserved_bytes) VALUES ('acme',16) ON CONFLICT (tenant) DO UPDATE SET reserved_bytes=16")
        .execute(c.f.store.pool.fixture_driver()).await.unwrap();
	let (status, body) = request(&c.app, &c.token, "POST", &path_for_patch(run.id),
        json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"unrelated.txt":null},"patch":"*** Begin Patch\n*** Add File: unrelated.txt\n+abcd\n*** End Patch"})).await;
	c.close().await;
	assert_eq!(
		status, 409,
		"unrelated upload consumed a transfer reservation: {body}"
	);
	assert_eq!(body["error"]["code"], "STORAGE_QUOTA");
}
