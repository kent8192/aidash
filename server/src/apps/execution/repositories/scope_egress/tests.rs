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
	async fn finish(&self, _: u16, _: u64, _: Option<&str>) -> aidash_application::Result<()> {
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
	};
	let durable = DurableScopes::new(federation.store.clone(), Arc::new(Authority(scope.clone())));
	assert!(durable.resolve("invalid").await.is_err());
	durable.resolve("credential").await.unwrap();
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
		.debit(&scope, Direction::Sent, 3, day)
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
	durable.admit_connection(&scope).await.unwrap();
	assert!(
		durable
			.admit_connection(&scope)
			.await
			.unwrap_err()
			.to_string()
			.contains("egress_suspended")
	);
	assert!(durable.check_active(&scope).await.is_err());
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
}
