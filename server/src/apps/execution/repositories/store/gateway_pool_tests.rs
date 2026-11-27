use super::Store;
use aidash_domain::nodes::DeploymentId;
use std::time::Duration;

#[tokio::test]
async fn gateway_runtime_views_reuse_the_same_eight_connections() {
	let pool = sqlx::postgres::PgPoolOptions::new()
		.max_connections(8)
		.acquire_timeout(Duration::from_secs(1))
		.connect_lazy("postgres://localhost/unused")
		.unwrap();
	let store = Store::from_gateway_pool(
		pool.clone().into(),
		DeploymentId::parse("aidash://gateway-test").unwrap(),
	)
	.await
	.unwrap();
	let isolated = store.isolated_pool().await.unwrap();
	let worker = store.worker_pool().await.unwrap();
	let recovery = store.recovery_pool().await.unwrap();
	let views = [&store, &isolated, &worker, &recovery];
	pool.close().await;
	for view in views {
		for handle in [
			&view.pool,
			&view.control_pool,
			&view.restoration_pool,
			&view.scratch_upload_pool,
		] {
			assert_eq!(handle.usage().max, 8);
			assert!(handle.driver().is_closed());
		}
	}
}
