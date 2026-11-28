use super::reference_set_tests::{add, file};
use super::*;

#[rstest::rstest]
#[tokio::test]
async fn upload_reservations_use_the_callers_tenant_scope(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let pool = c.f.store.pool.fixture_driver();
	// A restrictive policy makes the otherwise invisible side-transaction
	// scope observable at the same RLS boundary used in production.
	let owner = common::owner_pool(pool).await;
	sqlx::query("CREATE POLICY fixture_tenant_only ON core_blob_uploads AS RESTRICTIVE FOR ALL USING (tenant = (SELECT public.aidash_scope_tenant())) WITH CHECK (tenant = (SELECT public.aidash_scope_tenant()))")
        .execute(&owner).await.unwrap();
	owner.close().await;
	let (status, body) = add(&c, &c.token, vec![file("scoped.txt", "scoped\n")]).await;
	let released = tokio::time::timeout(std::time::Duration::from_secs(8), async {
		loop {
			let scope = aidash_application::tenancy::TenancyScope::tenant("acme").unwrap();
			let mut tx = c.f.store.pool.begin(&scope).await.unwrap();
			let rows: i64 = aidash_server::database::native::query_scalar(
				"SELECT count(*) FROM core_blob_uploads WHERE tenant='acme'",
			)
			.scalar_one(&mut *tx)
			.await
			.unwrap();
			tx.commit().await.unwrap();
			if rows == 0 {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
	})
	.await
	.is_ok();
	c.close().await;
	assert_eq!(
		status, 200,
		"upload side transactions must retain tenant RLS: {body}"
	);
	assert_eq!(body["references"].as_array().unwrap().len(), 1);
	assert!(released, "reservation cleanup must use tenant RLS too");
}
