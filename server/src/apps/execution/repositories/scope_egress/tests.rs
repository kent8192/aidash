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
/// Cannot be reached on its first charge, then commits what it is given and
/// counts it.
#[derive(Default)]
struct FlakyAuditMeter {
	calls: std::sync::atomic::AtomicU64,
	committed: std::sync::atomic::AtomicU64,
}
impl FlakyAuditMeter {
	fn committed(&self) -> u64 {
		self.committed.load(std::sync::atomic::Ordering::Relaxed)
	}
}
#[async_trait]
impl Meter for FlakyAuditMeter {
	async fn charge(&self, _: Direction, bytes: usize) -> aidash_application::Result<()> {
		use std::sync::atomic::Ordering::Relaxed;
		if self.calls.fetch_add(1, Relaxed) == 0 {
			return Err(aidash_application::Error::External(
				"audit unavailable".into(),
			));
		}
		self.committed.fetch_add(bytes as u64, Relaxed);
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
/// A scope authority that cannot be asked about the scope on its first call.
struct FlakyAuthority {
	inner: Authority,
	calls: std::sync::atomic::AtomicU64,
}
#[async_trait]
impl ScopeAuthority for FlakyAuthority {
	async fn authenticate(&self, credential: &str) -> aidash_application::Result<ScopeSettings> {
		self.inner.authenticate(credential).await
	}
	async fn current(&self, id: &ScopeId) -> aidash_application::Result<ScopeSettings> {
		if self
			.calls
			.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
			== 0
		{
			return Err(aidash_application::Error::External(
				"authority unavailable".into(),
			));
		}
		self.inner.current(id).await
	}
	async fn admit_external(
		&self,
		credential: &str,
		scope: &ScopeId,
		service: &str,
		method: &str,
		path: &str,
		digest: &str,
	) -> aidash_application::Result<Admission> {
		self.inner
			.admit_external(credential, scope, service, method, path, digest)
			.await
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
		error: None,
		wait_ms: None,
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
	let over_limit = ConnectionRecord {
		id: Uuid::new_v4(),
		outcome: "refused".into(),
		error: Some("egress_tenant_tunnel_limit".into()),
		wait_ms: Some(10_000),
		..connection.clone()
	};
	durable.record(over_limit.clone()).await.unwrap();
	let cause: (String, Option<String>, Option<i64>) = sqlx::query_as(
		"SELECT outcome, error, wait_ms FROM egress_scope_connections WHERE id = $1",
	)
	.bind(over_limit.id)
	.fetch_one(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		cause,
		(
			"refused".into(),
			Some("egress_tenant_tunnel_limit".into()),
			Some(10_000)
		)
	);
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
	let total = async || -> i64 {
		sqlx::query_scalar(
			"SELECT received_bytes FROM egress_scope_daily WHERE scope_id = $1 AND service = ''",
		)
		.bind(scope.id.scratch_space)
		.fetch_one(federation.store.pool.fixture_driver())
		.await
		.unwrap()
	};
	let audit = Arc::new(FlakyAuditMeter::default());
	let failed_audit =
		ScopedMeter::new(durable.clone(), scope.clone(), "maps", None, audit.clone());
	assert!(failed_audit.charge(Direction::Received, 1).await.is_err());
	assert_eq!(
		total().await,
		7,
		"audit failure must not refund forwarded bytes"
	);
	// The gateway charges the same bytes again after a failure that is not a
	// decision. The scope has them already; only the audit is still owed.
	failed_audit.charge(Direction::Received, 1).await.unwrap();
	assert_eq!(
		total().await,
		7,
		"the retry does not charge the scope twice"
	);
	assert_eq!(audit.committed(), 1, "the audit counts the bytes once");
	// The reverse: the scope could not be asked after the audit had the bytes.
	let audit = Arc::new(FlakyAuditMeter::default());
	audit.calls.store(1, std::sync::atomic::Ordering::Relaxed);
	let unreachable = ScopedMeter::new(
		DurableScopes::new(
			federation.store.clone(),
			Arc::new(FlakyAuthority {
				inner: Authority(scope.clone()),
				calls: Default::default(),
			}),
		),
		scope.clone(),
		"maps",
		None,
		audit.clone(),
	);
	assert!(unreachable.charge(Direction::Received, 1).await.is_err());
	assert_eq!((total().await, audit.committed()), (7, 1));
	unreachable.charge(Direction::Received, 1).await.unwrap();
	assert_eq!(
		(total().await, audit.committed()),
		(8, 1),
		"the retry charges the scope and not the audit again"
	);
	// What both sides acknowledged is not carried over to the next charge.
	unreachable.charge(Direction::Received, 1).await.unwrap();
	assert_eq!((total().await, audit.committed()), (9, 2));
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

const KEY: &str = "scope-test-key";

async fn insert_space(
	federation: &crate::federation::Federation,
	tenant: &str,
	settings: aidash_domain::scratch::ScratchSettings,
) -> Uuid {
	let now = Utc::now();
	let space = aidash_domain::scratch::ScratchSpace {
		id: Uuid::new_v4(),
		node: tenant_roots::fixture_node_name(tenant),
		name: "scope".to_owned(),
		creator: "owner".to_owned(),
		state: aidash_domain::scratch::ScratchState::Active,
		revision: 0,
		wipe_generation: 0,
		settings,
		members: Default::default(),
		created_at: now,
		last_used_at: now,
	};
	let mut tx = federation
		.store
		.pool
		.begin(&TenancyScope::tenant(tenant).unwrap())
		.await
		.unwrap();
	crate::apps::workspaces::repositories::scratch::insert(&mut *tx, tenant, &space)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	space.id
}

async fn delete_space(federation: &crate::federation::Federation, tenant: &str, space: Uuid) {
	let mut tx = federation
		.store
		.pool
		.begin(&TenancyScope::tenant(tenant).unwrap())
		.await
		.unwrap();
	crate::apps::workspaces::repositories::scratch::delete(&mut *tx, tenant, space)
		.await
		.unwrap();
	tx.commit().await.unwrap();
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn a_scope_credential_opens_its_live_scratch_space_until_the_space_is_deleted(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let mut chosen = aidash_domain::scratch::ScratchSettings {
		daily_sent_bytes: 11,
		daily_received_bytes: 22,
		..Default::default()
	};
	chosen.allowed_hosts = aidash_domain::scratch::HostAllowlist::Hosts(
		["registry.npmjs.org", "not-allowed-by-the-tenant.example"]
			.map(str::to_owned)
			.into(),
	);
	let space = insert_space(&federation, "acme", chosen).await;
	let policy = federation.config.scratch_policy.clone();
	let authority = Arc::new(ScratchAuthority::with_key(
		federation.store.clone(),
		policy,
		KEY,
	));
	let credential = authority.credential("acme", space).unwrap();
	let durable = DurableScopes::new(federation.store.clone(), authority.clone());

	let resolved = durable.resolve(&credential).await.unwrap();
	assert_eq!(resolved.id.tenant, "acme");
	assert_eq!(resolved.id.scratch_space, space);
	assert_eq!(
		resolved.id.node,
		tenant_roots::fixture_node_name("acme").to_string()
	);
	assert_eq!(resolved.budget.sent_bytes, 11);
	assert_eq!(resolved.budget.received_bytes, 22);
	assert_eq!(
		resolved.allowed_hosts.hosts,
		vec!["registry.npmjs.org".to_owned()],
		"the Tenant's limit narrows the space's own hosts"
	);
	durable.admit_connection(&resolved).await.unwrap();
	durable.check_active(&resolved, &credential).await.unwrap();

	let foreign = ScratchAuthority::with_key(
		federation.store.clone(),
		federation.config.scratch_policy.clone(),
		"another-key",
	)
	.credential("acme", space)
	.unwrap();
	assert!(matches!(
		durable.resolve(&foreign).await,
		Err(aidash_application::Error::Unauthorized)
	));
	assert!(matches!(
		durable
			.admit_external(&credential, "maps", "GET", "/", "digest")
			.await,
		Err(aidash_application::Error::Forbidden)
	));

	delete_space(&federation, "acme", space).await;
	assert!(matches!(
		durable.resolve(&credential).await,
		Err(aidash_application::Error::Unauthorized)
	));
}

/// A deletion is recorded for the Tenant or the Node and leaves the Scratch
/// Space Active until its data is purged, so the credential has to look.
#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn deleting_the_tenant_or_the_node_closes_the_scope_credential_until_it_is_restored(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let space = insert_space(&federation, "acme", Default::default()).await;
	let authority = Arc::new(ScratchAuthority::with_key(
		federation.store.clone(),
		federation.config.scratch_policy.clone(),
		KEY,
	));
	let credential = authority.credential("acme", space).unwrap();
	let durable = DurableScopes::new(federation.store.clone(), authority);
	let open = durable.resolve(&credential).await.unwrap();
	let node = tenant_roots::fixture_node_name("acme").to_string();
	let pool = federation.store.pool.fixture_driver();
	let closed = async || {
		// Admission, the periodic recheck and the live read behind a debit.
		assert!(matches!(
			durable.resolve(&credential).await,
			Err(aidash_application::Error::Forbidden)
		));
		assert!(matches!(
			durable.check_active(&open, &credential).await,
			Err(aidash_application::Error::Forbidden)
		));
		assert!(matches!(
			durable.admit_connection(&open).await,
			Err(aidash_application::Error::Forbidden)
		));
	};

	sqlx::query(
		"INSERT INTO tenant_deletions \
		 (id, tenant, deleted_at, deleted_by_kind, purge_after, purge_failures) \
		 VALUES ($1, 'acme', now(), 'deployment_administrator', now() + interval '30 days', 0)",
	)
	.bind(Uuid::new_v4())
	.execute(pool)
	.await
	.unwrap();
	closed().await;
	sqlx::query(
		"UPDATE tenant_deletions SET restored_at = now(), \
		 restored_by_kind = 'deployment_administrator' WHERE tenant = 'acme'",
	)
	.execute(pool)
	.await
	.unwrap();
	durable.resolve(&credential).await.unwrap();
	durable.check_active(&open, &credential).await.unwrap();

	sqlx::query(
		"INSERT INTO node_deletions \
		 (id, tenant, node, deleted_at, deleted_by_kind, purge_after, purge_failures) \
		 VALUES ($1, 'acme', $2, now(), 'deployment_administrator', now() + interval '30 days', 0)",
	)
	.bind(Uuid::new_v4())
	.bind(&node)
	.execute(pool)
	.await
	.unwrap();
	closed().await;
	sqlx::query(
		"UPDATE node_deletions SET restored_at = now(), \
		 restored_by_kind = 'deployment_administrator' WHERE tenant = 'acme' AND node = $1",
	)
	.bind(&node)
	.execute(pool)
	.await
	.unwrap();
	durable.resolve(&credential).await.unwrap();
}

/// Reads until the gateway closes the connection.
async fn proxy_request(address: std::net::SocketAddr, request: String) -> String {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
	client.write_all(request.as_bytes()).await.unwrap();
	let mut response = String::new();
	client.read_to_string(&mut response).await.unwrap();
	response
}

fn connect_request(target: &str, credential: &str) -> String {
	format!(
		"CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Authorization: Bearer {credential}\r\nConnection: close\r\n\r\n"
	)
}

#[rstest::rstest]
#[tokio::test]
#[timeout(std::time::Duration::from_secs(180))]
async fn a_real_listener_records_host_connections_in_the_database(
	#[future] database: DatabaseFixture,
) {
	let database = database.await;
	let context = reinhardt::InjectionContext::builder(reinhardt::SingletonScope::new()).build();
	let settings = settings::settings_for(&database.url);
	let federation = crate::bootstrap::initialize(&context, &settings, database.connection.clone())
		.await
		.unwrap();
	tenant_roots::tenant_root(&federation, "acme").await;
	let space = insert_space(&federation, "acme", Default::default()).await;
	let authority = Arc::new(ScratchAuthority::with_key(
		federation.store.clone(),
		federation.config.scratch_policy.clone(),
		KEY,
	));
	let credential = authority.credential("acme", space).unwrap();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = listener.local_addr().unwrap();
	let (stop, receiver) = tokio::sync::watch::channel(false);
	let gateway = tokio::spawn(aidash_integrations::egress::serve_with_scopes(
		listener,
		Arc::new(crate::apps::execution::repositories::egress::Gateway::new(
			federation.store.clone(),
		)),
		Some(Arc::new(DurableScopes::new(
			federation.store.clone(),
			authority,
		))),
		Default::default(),
		receiver,
	));

	let refused = proxy_request(address, connect_request("registry.npmjs.org:443", "wrong")).await;
	assert!(refused.contains("proxy_auth_required"), "{refused}");
	let forbidden = proxy_request(
		address,
		connect_request("not-allowed.example:443", &credential),
	)
	.await;
	assert!(forbidden.contains("egress_host_forbidden"), "{forbidden}");
	// Port 0 is held to the record's check constraint from its first write.
	for port in [22, 0] {
		let invalid = proxy_request(
			address,
			connect_request(&format!("registry.npmjs.org:{port}"), &credential),
		)
		.await;
		assert!(invalid.contains("invalid_proxy_target"), "{invalid}");
	}
	stop.send(true).unwrap();
	gateway.await.unwrap().unwrap();

	let records: Vec<(String, i32, String, Option<String>)> = sqlx::query_as(
		"SELECT host, port, outcome, error FROM egress_scope_connections WHERE scope_id = $1 ORDER BY created_at",
	)
	.bind(space)
	.fetch_all(federation.store.pool.fixture_driver())
	.await
	.unwrap();
	let invalid = |port| {
		(
			"registry.npmjs.org".to_owned(),
			port,
			"invalid_proxy_target".to_owned(),
			Some("invalid_proxy_target".to_owned()),
		)
	};
	assert_eq!(
		records,
		vec![
			(
				"not-allowed.example".to_owned(),
				443,
				"refused".to_owned(),
				Some("egress_host_forbidden".to_owned())
			),
			invalid(22),
			invalid(0),
		]
	);
}
