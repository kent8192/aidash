//! Concurrent restore requests must not re-enter retirement after the winner restores.
use super::*;
use aidash_application::plan_connections::{self as plans, Owner};
use aidash_application::provider_access::KeyMaterialReader;
use aidash_application::provider_credentials::Store;
use async_trait::async_trait;
use secrecy::SecretString;
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

#[derive(Default)]
struct Retirement {
	calls: AtomicUsize,
	arrived: Notify,
	release: Notify,
}
#[async_trait]
impl plans::Repository for Retirement {
	async fn begin(&self, _: &Owner) -> aidash_application::Result<Box<dyn plans::Scope>> {
		unreachable!()
	}
	async fn owner_of(&self, _: Uuid) -> aidash_application::Result<Option<Owner>> {
		unreachable!()
	}
	async fn owners(&self, _: &str) -> aidash_application::Result<Vec<Owner>> {
		if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
			self.arrived.notify_one();
			self.release.notified().await;
		}
		Ok(vec![])
	}
}
// No connection exists in this scenario. Any use of tokens/provider I/O would
// cross the fixture's expected retirement boundary and fail the test.
struct Unused;
#[async_trait]
impl Store for Unused {
	fn resource(&self, _: Uuid) -> String {
		unreachable!()
	}
	async fn create(&self, _: &str, _: Uuid) -> aidash_application::Result<()> {
		unreachable!()
	}
	async fn add_version(
		&self,
		_: &str,
		_: &str,
		_: &SecretString,
	) -> aidash_application::Result<String> {
		unreachable!()
	}
	async fn versions(&self, _: &str, _: &str) -> aidash_application::Result<Vec<String>> {
		unreachable!()
	}
	async fn disable(&self, _: &str, _: &str) -> aidash_application::Result<()> {
		unreachable!()
	}
	async fn destroy(&self, _: &str, _: &str) -> aidash_application::Result<()> {
		unreachable!()
	}
	async fn delete(&self, _: &str, _: &str) -> aidash_application::Result<()> {
		unreachable!()
	}
}
#[async_trait]
impl KeyMaterialReader for Unused {
	async fn read(&self, _: &str, _: &str, _: &str) -> aidash_application::Result<SecretString> {
		unreachable!()
	}
}
#[async_trait]
impl plans::Authorization for Unused {
	async fn exchange(
		&self,
		_: plans::ExchangeRequest<'_>,
	) -> aidash_application::Result<plans::Issued> {
		unreachable!()
	}
	async fn verify_id_token(
		&self,
		_: &SecretString,
		_: &str,
		_: &str,
	) -> aidash_application::Result<String> {
		unreachable!()
	}
	async fn refresh(
		&self,
		_: &str,
		_: &SecretString,
	) -> std::result::Result<plans::Issued, plans::RefreshFailure> {
		unreachable!()
	}
	async fn revoke(&self, _: &str, _: &SecretString) -> aidash_application::Result<()> {
		unreachable!()
	}
	async fn models(
		&self,
		_: &SecretString,
	) -> aidash_application::Result<Vec<aidash_domain::plan_connections::PlanModel>> {
		unreachable!()
	}
}

#[rstest::rstest]
#[tokio::test]
async fn overlapping_restores_retire_connections_only_once(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let mut d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("restore-owner").await;
	let (tenant, context) = d.tenant_of(&alice, "Restore race").await;
	assert_eq!(d.delete(&alice, &context, &tenant).await.0, 200);
	let retirement = Arc::new(Retirement::default());
	d.federation.store.plan_connections = Some(Arc::new(plans::Service {
		repository: retirement.clone(),
		store: Arc::new(Unused),
		reader: Arc::new(Unused),
		authorization: Arc::new(Unused),
	}));
	d.app = common::application(d.federation.clone()).await;
	let mut first = Box::pin(d.restore(&alice, &context, &tenant));
	tokio::select! {
		_ = retirement.arrived.notified() => {},
		result = &mut first => panic!("restore ended before retirement was released: {result:?}"),
	}
	let mut second = Box::pin(d.restore(&alice, &context, &tenant));
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            tokio::select! {
                result = &mut second => panic!("overlapping restore bypassed serialization: {result:?}"),
                _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                    // Observe a real PostgreSQL lock wait, rather than assume a
                    // request has reached the boundary after a fixed sleep.
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND objid=((hashtextextended($1,391351) & 4294967295)::oid) AND classid=(((hashtextextended($1,391351) >> 32) & 4294967295)::oid))")
                        .bind(&tenant).fetch_one(d.federation.store.pool.fixture_driver()).await.unwrap();
                    if waiting { break; }
                }
            }
        }
    }).await.expect("the second restore must wait for the first retirement");
	assert_eq!(retirement.calls.load(Ordering::SeqCst), 1);
	retirement.release.notify_one();
	let (first, second) = tokio::join!(first, second);
	assert_eq!(first.0, 200, "{first:?}");
	assert_eq!(second.0, 409, "{second:?}");
	assert_eq!(
		retirement.calls.load(Ordering::SeqCst),
		1,
		"the losing restore cannot retire a restored Tenant's new connections"
	);
	d.finish().await;
}
