use super::*;

#[rstest::rstest]
#[tokio::test]
async fn concurrent_same_digest_reservations_do_not_double_count_retained_capacity(
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
	profile.retained_bytes = used as u64 + 10;
	profile.working_bytes = profile.retained_bytes;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app.context.set_singleton(c.f.clone());
	let pool = c.f.store.pool.fixture_driver();
	let prior = b"used!";
	fixture_blob(
		pool,
		&c.root,
		&aidash_server::capabilities::objects::digest(prior),
		prior,
	)
	.await;
	sqlx::query("INSERT INTO core_quotas (tenant,used_bytes) VALUES ('acme',5) ON CONFLICT (tenant) DO UPDATE SET used_bytes=core_quotas.used_bytes+5")
        .execute(pool).await.unwrap();
	let other = Uuid::new_v4();
	let digest = aidash_server::capabilities::objects::digest(b"abc\n");
	sqlx::query("INSERT INTO core_blob_uploads (id,tenant,size,digest) VALUES ($1,'acme',4,$2)")
		.bind(other)
		.bind(&digest)
		.execute(pool)
		.await
		.unwrap();
	let mut owner = pool.begin().await.unwrap();
	sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
		.bind(format!("core-object:{other}"))
		.execute(&mut *owner)
		.await
		.unwrap();
	let (status, result) = request(&c.app, &c.token, "POST", &path_for_patch(run.id),
        json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"same.txt":null},"patch":"*** Begin Patch\n*** Add File: same.txt\n+abc\n*** End Patch"})).await;
	owner.rollback().await.unwrap();
	c.close().await;
	assert_eq!(
		status, 200,
		"identical in-flight content must fit retained capacity: {result}"
	);
}
