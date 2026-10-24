//! Concurrent restore requests must not re-enter retirement after the winner restores.
use super::*;
use aidash_application::plan_connections::{self as plans, Owner};
use aidash_application::provider_access::KeyMaterialReader;
use aidash_application::provider_credentials::Store;
use async_trait::async_trait;
use reinhardt::query::{Alias, Expr, ExprTrait, IntoIden, PostgresQueryBuilder, Query, SimpleExpr};
use secrecy::SecretString;
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

struct Retirement {
	control: aidash_server::database::native::Pool,
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
	async fn owners(&self, tenant: &str) -> aidash_application::Result<Vec<Owner>> {
		let tx = self
			.control
			.begin(&aidash_application::tenancy::TenancyScope::tenant(tenant)?)
			.await
			.map_err(aidash_application::Error::from)?;
		if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
			self.arrived.notify_one();
			self.release.notified().await;
		}
		tx.rollback()
			.await
			.map_err(aidash_application::Error::from)?;
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
#[case::ordinary_control_capacity(false)]
#[case::single_control_connection(true)]
#[tokio::test]
async fn overlapping_restores_retire_connections_only_once(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[case] single_control: bool,
) {
	let mut d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("restore-owner").await;
	let (tenant, context) = d.tenant_of(&alice, "Restore race").await;
	assert_eq!(d.delete(&alice, &context, &tenant).await.0, 200);
	if single_control {
		let pool = &d.federation.store.control_pool;
		d.federation.store.control_pool = pool
			.options()
			.clone()
			.max_connections(1)
			.min_connections(0)
			.acquire_timeout(std::time::Duration::from_secs(2))
			.connect_with(pool.connect_options().as_ref().clone())
			.await
			.unwrap()
			.into();
	}
	let retirement = Arc::new(Retirement {
		control: d.federation.store.control_pool.clone(),
		calls: Default::default(),
		arrived: Default::default(),
		release: Default::default(),
	});
	d.federation.store.plan_connections = Some(Arc::new(plans::Service {
		repository: retirement.clone(),
		store: Arc::new(Unused),
		reader: Arc::new(Unused),
		authorization: Arc::new(Unused),
	}));
	d.app = common::application(d.federation.clone()).await;
	let mut first = Box::pin(d.restore(&alice, &context, &tenant));
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		tokio::select! {
			_ = retirement.arrived.notified() => {},
			result = &mut first => panic!("restore ended before retirement was released: {result:?}"),
		}
	})
	.await
	.expect("retirement must acquire its own control connection");
	let mut second = Box::pin(d.restore(&alice, &context, &tenant));
	let restore_key = SimpleExpr::FunctionCall(
		"hashtextextended".into_iden(),
		vec![Expr::value(&tenant).into(), Expr::value(391_351_i64).into()],
	);
	let database = Query::select()
		.column(Alias::new("oid"))
		.from(Alias::new("pg_database"))
		.and_where(
			Expr::col(Alias::new("datname")).eq(SimpleExpr::FunctionCall(
				"current_database".into_iden(),
				Vec::new(),
			)),
		)
		.to_owned();
	let waiting_sql = Query::select()
		.expr(Expr::exists(
			Query::select()
				.expr(Expr::value(1_i32))
				.from(Alias::new("pg_locks"))
				.and_where(Expr::col(Alias::new("locktype")).eq(Expr::value("advisory")))
				.and_where(Expr::col(Alias::new("granted")).eq(Expr::value(false)))
				.and_where(Expr::col(Alias::new("database")).eq(Expr::subquery(database)))
				.and_where(
					Expr::col(Alias::new("objid")).eq(restore_key
						.clone()
						.bit_and(Expr::value(i64::from(u32::MAX)))
						.cast_as("oid")),
				)
				.and_where(
					Expr::col(Alias::new("classid")).eq(restore_key
						.right_shift(Expr::value(32_i32))
						.bit_and(Expr::value(i64::from(u32::MAX)))
						.cast_as("oid")),
				)
				.to_owned(),
		))
		.to_string(PostgresQueryBuilder);
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			tokio::select! {
				result = &mut second => panic!("overlapping restore bypassed serialization: {result:?}"),
				_ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
					// Observe a real PostgreSQL lock wait, rather than assume a
					// request has reached the boundary after a fixed sleep.
					let waiting: bool = sqlx::query_scalar(&waiting_sql)
						.fetch_one(d.federation.store.pool.fixture_driver()).await.unwrap();
					if waiting { break; }
				}
			}
		}
	})
	.await
	.expect("the second restore must wait for the first retirement");
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
