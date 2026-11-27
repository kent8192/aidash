use super::*;
use futures_util::poll;
use reinhardt::StatusCode;
use serde_json::Value;

async fn occupy(pool: &Arc<FairShare>, tenant: &str, count: usize) -> Vec<Permit> {
	let tenant = TenantName::new(tenant).unwrap();
	let mut permits = Vec::new();
	for _ in 0..count {
		permits.push(pool.acquire(Some(tenant.clone())).await.unwrap());
	}
	permits
}

#[tokio::test]
async fn new_tenant_gets_the_next_work_slot_when_the_previous_tenant_filled_the_pool() {
	let pool = Arc::new(FairShare::new(10));
	let mut held = occupy(&pool, "first", 10).await;
	let mut first = Box::pin(pool.acquire(Some(TenantName::new("first").unwrap())));
	let mut second = Box::pin(pool.acquire(Some(TenantName::new("second").unwrap())));
	assert!(poll!(&mut first).is_pending());
	assert!(poll!(&mut second).is_pending());

	drop(held.pop());

	let std::task::Poll::Ready(Ok(second_permit)) = poll!(&mut second) else {
		panic!("the second Tenant should get the released slot");
	};
	assert!(poll!(&mut first).is_pending());
	drop(second_permit);
	assert!(poll!(&mut first).is_ready());
}

#[tokio::test(start_paused = true)]
async fn fair_share_wait_returns_503_with_the_current_share() {
	let pool = Arc::new(FairShare::new(10));
	let first = occupy(&pool, "first", 8).await;
	let _second = occupy(&pool, "second", 1).await;
	let _system = pool.acquire(None).await.unwrap();
	let tenant = TenantName::new("first").unwrap();
	let start = tokio::time::Instant::now();

	let error = pool
		.acquire(Some(tenant))
		.await
		.err()
		.expect("over-share wait expires");
	let response = crate::Error::from(error).http_response();
	let body: Value = serde_json::from_slice(&response.body).unwrap();

	assert_eq!(start.elapsed(), Duration::from_secs(10));
	assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
	assert_eq!(body["code"], FAIR_SHARE_CODE);
	let message = body["error"].as_str().unwrap();
	assert!(message.contains("Work pool has 10 connections"));
	assert!(message.contains("Tenant's share is 5"));
	drop(first);
}

#[tokio::test]
async fn pressure_stays_active_until_usage_falls_below_seventy_percent() {
	let pool = Arc::new(FairShare::new(10));
	let mut first = occupy(&pool, "first", 7).await;
	let _second = occupy(&pool, "second", 1).await;
	// At 80%, the over-share Tenant can still take the next free slot.
	let ninth = pool
		.acquire(Some(TenantName::new("first").unwrap()))
		.await
		.unwrap();
	first.push(ninth);
	drop(first.pop());

	let mut pending = Box::pin(pool.acquire(Some(TenantName::new("first").unwrap())));
	assert!(poll!(&mut pending).is_pending());
	drop(first.pop());
	assert!(poll!(&mut pending).is_pending());
	drop(first.pop());
	assert!(matches!(poll!(&mut pending), std::task::Poll::Ready(Ok(_))));
}
