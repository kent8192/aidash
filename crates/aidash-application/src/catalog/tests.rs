use super::*;
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Upstream {
	calls: AtomicUsize,
	failing: AtomicBool,
}

#[async_trait]
impl ModelCatalog for Upstream {
	async fn models(&self) -> Result<Vec<CatalogModel>> {
		self.calls.fetch_add(1, Ordering::SeqCst);
		tokio::task::yield_now().await;
		if self.failing.load(Ordering::SeqCst) {
			return Err(crate::Error::External("OpenRouter is down".into()));
		}
		Ok(vec![])
	}
}

fn at(seconds: i64) -> DateTime<Utc> {
	DateTime::<Utc>::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
}

#[tokio::test]
async fn concurrent_callers_share_one_fetch_per_period() {
	let cache = Arc::new(CatalogCache::new());
	let upstream = Arc::new(Upstream::default());
	let callers: Vec<_> = (0..16)
		.map(|_| {
			let (cache, upstream) = (cache.clone(), upstream.clone());
			tokio::spawn(async move { cache.get(upstream.as_ref(), || at(0)).await })
		})
		.collect();
	for caller in callers {
		assert!(!caller.await.unwrap().unwrap().stale);
	}
	assert_eq!(upstream.calls.load(Ordering::SeqCst), 1);
	cache.get(upstream.as_ref(), || at(599)).await.unwrap();
	assert_eq!(upstream.calls.load(Ordering::SeqCst), 1);
	cache.get(upstream.as_ref(), || at(600)).await.unwrap();
	assert_eq!(upstream.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_failed_refresh_serves_the_last_catalog_marked_stale_for_a_day() {
	let cache = CatalogCache::new();
	let upstream = Upstream::default();
	cache.get(&upstream, || at(0)).await.unwrap();
	upstream.failing.store(true, Ordering::SeqCst);
	let stale = cache.get(&upstream, || at(601)).await.unwrap();
	assert!(stale.stale);
	assert_eq!(stale.fetched_at, at(0));
	assert_eq!(upstream.calls.load(Ordering::SeqCst), 2);
	// Within the retry floor nobody calls OpenRouter again.
	cache.get(&upstream, || at(620)).await.unwrap();
	assert_eq!(upstream.calls.load(Ordering::SeqCst), 2);
	assert!(cache.get(&upstream, || at(24 * 3600)).await.is_err());
}

#[tokio::test]
async fn without_a_catalog_the_upstream_failure_is_returned() {
	let cache = CatalogCache::new();
	let upstream = Upstream::default();
	upstream.failing.store(true, Ordering::SeqCst);
	assert!(matches!(
		cache.get(&upstream, || at(0)).await,
		Err(crate::Error::External(message)) if message == "OpenRouter is down"
	));
	assert!(cache.get(&upstream, || at(1)).await.is_err());
	assert_eq!(upstream.calls.load(Ordering::SeqCst), 1);
}
