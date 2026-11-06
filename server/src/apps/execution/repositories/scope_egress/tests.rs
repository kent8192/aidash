use super::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::egress_tests::{settings, tenant_roots};
use aidash_domain::egress::{AllowedHosts, AnomalyRule, DailyBudget};

struct Authority(ScopeSettings);
struct AuditMeter;
#[async_trait]
impl Meter for AuditMeter {
	async fn charge(&self, _: Direction, _: usize) -> aidash_application::Result<()> {
		Ok(())
	}
	async fn finish(
		&self,
		_: u16,
		_: RequestTimes,
		_: Option<&str>,
	) -> aidash_application::Result<()> {
		Ok(())
	}
}
struct FailedAuditMeter;
#[async_trait]
impl Meter for FailedAuditMeter {
	async fn charge(&self, _: Direction, _: usize) -> aidash_application::Result<()> {
		Err(aidash_application::Error::External(
			"audit unavailable".into(),
		))
	}
	async fn finish(
		&self,
		_: u16,
		_: RequestTimes,
		_: Option<&str>,
	) -> aidash_application::Result<()> {
		Ok(())
	}
}
#[async_trait]
impl ScopeAuthority for Authority {
	async fn authenticate(&self, credential: &str) -> aidash_application::Result<ScopeSettings> {
		if credential != "credential" {
			return Err(aidash_application::Error::Unauthorized);
		}
		Ok(self.0.clone())
	}
	async fn current(&self, id: &ScopeId) -> aidash_application::Result<ScopeSettings> {
		if id != &self.0.id {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(self.0.clone())
	}
	async fn admit_external(
		&self,
		credential: &str,
		scope: &ScopeId,
		service: &str,
		_: &str,
		_: &str,
		_: &str,
	) -> aidash_application::Result<Admission> {
		if credential != "credential" || scope != &self.0.id || service != "maps" {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(Admission {
			gateway_origin: "http://gateway.test".into(),
			destination: aidash_domain::keyless::Destination::PublicOnly,
			request: Uuid::new_v4(),
			operation: None,
			session: None,
			service: service.into(),
			definition: aidash_domain::egress::ExternalService {
				base_url: "https://maps.example/".into(),
				methods: vec!["GET".into()],
				secret: None,
				authentication: Default::default(),
				limits: Some(aidash_domain::egress::Limits {
					sent_bytes: 5,
					received_bytes: 5,
					requests: 1,
				}),
			},
			expires_at: Utc::now() + chrono::Duration::minutes(1),
			credential: None,
			route: None,
			meter: Arc::new(AuditMeter),
		})
	}
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn durable_scope_serializes_daily_budget_and_persists_stop_event(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	tenant_roots::tenant_root(&federation, "beta").await;
	let scope = ScopeSettings {
		id: ScopeId {
			tenant: "acme".into(),
			node: tenant_roots::fixture_node_name("acme").to_string(),
			scratch_space: Uuid::new_v4(),
		},
		allowed_hosts: AllowedHosts::default(),
		budget: DailyBudget {
			sent_bytes: 10,
			received_bytes: 10,
		},
		anomaly: AnomalyRule {
			connections_per_minute: 1,
		},
		operator_allows_all: false,
		tunnel_limits: Default::default(),
	};
	let durable = DurableScopes::new(federation.store.clone(), Arc::new(Authority(scope.clone())));
	assert!(durable.resolve("invalid").await.is_err());
	durable.resolve("credential").await.unwrap();
	let mut connection = ConnectionRecord {
		id: Uuid::new_v4(),
		scope: scope.id.clone(),
		host: "registry.npmjs.org".into(),
		port: 443,
		sent_bytes: 0,
		received_bytes: 0,
		duration_ms: 0,
		outcome: "interrupted".into(),
	};
	durable.record(connection.clone()).await.unwrap();
	connection.sent_bytes = 7;
	connection.outcome = "completed".into();
	durable.record(connection.clone()).await.unwrap();
	let recorded: (i64, i64) = sqlx::query_as("SELECT count(*), COALESCE(sum(sent_bytes), 0)::bigint FROM egress_scope_connections WHERE id = $1")
		.bind(connection.id).fetch_one(federation.store.pool.fixture_driver()).await.unwrap();
	assert_eq!(recorded, (1, 7));
	let invalid_target = ConnectionRecord {
		id: Uuid::new_v4(),
		port: 0,
		sent_bytes: 0,
		received_bytes: 0,
		outcome: "invalid_proxy_target".into(),
		..connection.clone()
	};
	durable.record(invalid_target).await.unwrap();
	let mut other = federation
		.store
		.pool
		.begin(&TenancyScope::tenant("beta").unwrap())
		.await
		.unwrap();
	let hidden = ScopeState::objects()
		.filter(ScopeState::field_scope_id().eq(scope.id.scratch_space))
		.all_with_executor(&mut *other)
		.await
		.unwrap();
	assert!(hidden.is_empty(), "another Tenant must not see scope state");
	other.commit().await.unwrap();
	let day = Utc::now().date_naive();
	let (left, right) = tokio::join!(
		durable.debit(&scope, Direction::Sent, 8, day),
		durable.debit(&scope, Direction::Sent, 8, day)
	);
	assert_ne!(
		left.is_ok(),
		right.is_ok(),
		"exactly one concurrent debit fits"
	);
	assert!(
		durable
			.debit(&scope, Direction::Sent, 3, day)
			.await
			.is_err()
	);
	durable
		.refund(&scope, Direction::Sent, 1, day)
		.await
		.unwrap();
	durable
		.debit(&scope, Direction::Sent, 2, day)
		.await
		.unwrap();
	let first = durable
		.admit_external("credential", "maps", "GET", "/", "digest")
		.await
		.unwrap();
	let second = durable
		.admit_external("credential", "maps", "GET", "/", "digest")
		.await
		.unwrap();
	first.meter.charge(Direction::Received, 3).await.unwrap();
	assert!(
		second
			.meter
			.charge(Direction::Received, 3)
			.await
			.unwrap_err()
			.to_string()
			.contains("egress_budget_exhausted")
	);
	let recorded: i64 = sqlx::query_scalar(
		"SELECT received_bytes FROM egress_scope_daily WHERE scope_id = $1 AND service = 'maps'",
	)
	.bind(scope.id.scratch_space)
	.fetch_one(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		recorded, 6,
		"already forwarded bytes remain charged after an over-limit debit"
	);
	let failed_audit = ScopedMeter {
		durable: durable.clone(),
		settings: scope.clone(),
		service: "maps".into(),
		limits: None,
		inner: Arc::new(FailedAuditMeter),
	};
	assert!(failed_audit.charge(Direction::Received, 1).await.is_err());
	let total: i64 = sqlx::query_scalar(
		"SELECT received_bytes FROM egress_scope_daily WHERE scope_id = $1 AND service = ''",
	)
	.bind(scope.id.scratch_space)
	.fetch_one(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(total, 7, "audit failure must not refund forwarded bytes");
	for _ in 0..2 {
		assert!(
			durable
				.admit_external("credential", "maps", "GET", "/", "digest")
				.await
				.err()
				.unwrap()
				.to_string()
				.contains("egress_budget_exhausted")
		);
	}
	durable.admit_connection(&scope).await.unwrap();
	assert!(
		durable
			.admit_connection(&scope)
			.await
			.unwrap_err()
			.to_string()
			.contains("egress_suspended")
	);
	assert!(durable.check_active(&scope, "credential").await.is_err());
	let events: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM egress_scope_events WHERE scope_id = $1 AND kind = 'suspended'",
	)
	.bind(scope.id.scratch_space)
	.fetch_one(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(events, 1);
	durable.lift_suspension(&scope.id).await.unwrap();
	durable.admit_connection(&scope).await.unwrap();
	durable
		.debit(&scope, Direction::Sent, 1, day)
		.await
		.unwrap();
	for _ in 0..2 {
		assert!(
			durable
				.admit_connection(&scope)
				.await
				.unwrap_err()
				.to_string()
				.contains("egress_budget_exhausted")
		);
	}
}

#[test]
fn storage_counter_ceiling_cannot_turn_a_budget_into_unlimited_traffic() {
	let row = ScopeDaily::build()
		.id(Uuid::new_v4())
		.tenant("acme".to_owned())
		.node("node".to_owned())
		.scope_id(Uuid::new_v4())
		.day("2026-01-01".to_owned())
		.service(String::new())
		.sent_bytes(i64::MAX)
		.received_bytes(0)
		.finish();
	assert!(budget_available(&row, u64::MAX, u64::MAX).is_err());
}
