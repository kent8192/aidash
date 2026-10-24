//! The billing core on PostgreSQL: meters, Billing Accounts, Tiers, the rules
//! for who may exist on Cloud, Run admission and the usage report.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../server/src/apps/execution/tests/support/legacy.rs"]
mod common;
#[path = "billing/real_allocation.rs"]
mod real_allocation;
use aidash_application::tenancy::TenancyScope;
use aidash_application::usage::{Admission, PodTime, StorageSample, UsageEvents, Verdict, Work};
use aidash_domain::identity::tenancy::TenancyRules;
use aidash_domain::offering::Offering;
use aidash_enterprise_server::billing::gate::Gate;
use aidash_enterprise_server::billing::meter::{Meter, cutoff};
use aidash_enterprise_server::billing::nodes::FixedNodes;
use aidash_enterprise_server::billing::payments::{
	FakePayments, Mirror, PaymentState, Status, Subscription,
};
use aidash_enterprise_server::billing::service::{Billing, Consented, Party};
use aidash_enterprise_server::billing::settings::{Settings, sandbox_defaults};
use aidash_enterprise_server::billing::store;
use aidash_server::config::OidcConfig;
use aidash_server::federation::Federation;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

const DEPLOYMENT_ADMINISTRATOR: &str = "deployment-administrator-execution-fixture";

#[derive(Debug, PartialEq)]
struct Coverage {
	id: Uuid,
	account_id: Uuid,
	starts_at: DateTime<Utc>,
	ends_at: Option<DateTime<Utc>>,
	consented_by_administrator: Uuid,
	consented_by_owner: Uuid,
}

type CoverageRow = (Uuid, Uuid, DateTime<Utc>, Option<DateTime<Utc>>, Uuid, Uuid);

fn settings() -> Arc<Settings> {
	Arc::new(Settings::from_value(sandbox_defaults()).unwrap())
}

/// One Cloud Deployment of the Enterprise build with direct Google sign-in.
struct Cloud {
	federation: Federation,
	app: common::TestApplication,
	url: String,
	database: String,
	_runtime: common::RuntimeFixture,
}

struct Person {
	name: String,
	identity: Uuid,
}

impl Cloud {
	async fn new(runtime: common::RuntimeFuture) -> Self {
		aidash_server::bootstrap::build::install(&aidash_enterprise_server::BUILD);
		let runtime = runtime.await;
		let (mut federation, url, database) = runtime.parts();
		federation.config.oidc = Some(OidcConfig {
			issuer: "https://accounts.google.com".into(),
			client_id: "aidash".into(),
			client_secret: "test-secret".into(),
			public_origin: "https://127.0.0.1:8080".into(),
			session_absolute_seconds: 43_200,
			session_idle_seconds: 1_800,
		});
		federation.config.tenancy = TenancyRules {
			creation_per_hour: 100,
			creation_total: 100,
			..TenancyRules::of(Offering::Cloud)
		};
		aidash_enterprise_server::billing::install_with(
			&mut federation.store,
			settings(),
			Offering::Cloud,
		);
		federation.config.tier_limits = aidash_enterprise_server::BUILD
			.extension
			.tier_limits(&federation.store)
			.unwrap();
		let app = common::application(federation.clone()).await;
		Self {
			federation,
			app,
			url,
			database,
			_runtime: runtime,
		}
	}

	/// A signed-in External Identity; its session and CSRF secrets are `name`.
	async fn person(&self, name: &str) -> Person {
		let identity = Uuid::new_v4();
		let pool = self.federation.store.pool.driver();
		sqlx::query(
			"INSERT INTO dashboard_identities (id, issuer, subject, last_valid_at, verified_email, display_name, display_observed_at) \
			 VALUES ($1, 'https://accounts.google.com', $2, clock_timestamp(), $3, $2, clock_timestamp())",
		)
		.bind(identity)
		.bind(name)
		.bind(format!("{name}@example.com"))
		.execute(pool)
		.await
		.unwrap();
		sqlx::query(
			"INSERT INTO dashboard_sessions (id, token_hash, csrf_hash, identity_id, created_at, last_activity_at, expires_at) \
			 VALUES ($1, $2, $3, $4, clock_timestamp(), clock_timestamp(), clock_timestamp() + interval '12 hours')",
		)
		.bind(Uuid::new_v4())
		.bind(common::browser_session_digest(name))
		.bind(Sha256::digest(name.as_bytes()).to_vec())
		.bind(identity)
		.execute(pool)
		.await
		.unwrap();
		Person {
			name: name.into(),
			identity,
		}
	}

	async fn send(
		&self,
		method: &str,
		path: &str,
		person: Option<&Person>,
		context: Option<&str>,
		body: Value,
	) -> (u16, Value, http::HeaderMap, String) {
		let mut request = self
			.app
			.api_http
			.request(
				http::Method::from_bytes(method.as_bytes()).unwrap(),
				&self.app.url(path),
			)
			.header("content-type", "application/json");
		match person {
			Some(person) => {
				request = request
					.header("cookie", format!("__Host-aidash-session={}", person.name))
					.header("origin", "https://127.0.0.1:8080")
					.header("x-aidash-csrf", person.name.as_str());
			}
			None => {
				request = request.header(
					"authorization",
					format!("Bearer {DEPLOYMENT_ADMINISTRATOR}"),
				);
			}
		}
		if let Some(context) = context {
			request = request.header("x-aidash-context", context);
		}
		let request = if body.is_null() {
			request
		} else {
			request.json(&body)
		};
		let response = request.send().await.unwrap();
		let status = response.status().as_u16();
		let headers = response.headers().clone();
		let text = String::from_utf8_lossy(response.body()).into_owned();
		let body = serde_json::from_str(&text).unwrap_or(Value::Null);
		(status, body, headers, text)
	}

	async fn call(
		&self,
		person: &Person,
		method: &str,
		path: &str,
		context: Option<&str>,
		body: Value,
	) -> (u16, Value) {
		let (status, body, ..) = self.send(method, path, Some(person), context, body).await;
		(status, body)
	}

	/// The person's enabled Mapping into `tenant`, as a context selector.
	async fn context(&self, person: &Person, tenant: &str) -> String {
		let (status, session) = self
			.call(person, "GET", "/auth/session", None, Value::Null)
			.await;
		assert_eq!(status, 200, "{session}");
		let mapping = session["mappings"]
			.as_array()
			.unwrap()
			.iter()
			.find(|mapping| mapping["tenant"] == tenant)
			.unwrap_or_else(|| panic!("no Mapping into {tenant}: {session}"));
		format!("mapping:{tenant}:{}", mapping["id"].as_str().unwrap())
	}

	/// A Tenant the person created, with the person's Mapping into it.
	async fn tenant_of(&self, person: &Person) -> (String, String) {
		let (status, created) = self
			.call(
				person,
				"POST",
				"/api/tenants",
				None,
				json!({"label":"Team"}),
			)
			.await;
		assert_eq!(status, 201, "{created}");
		let tenant = created["name"].as_str().unwrap().to_owned();
		// These metering fixtures simulate historical deployments. Keep the
		// permanent Tenant birth before their explicitly dated usage/history.
		sqlx::query("UPDATE tenants SET created_at = '1970-01-01T00:00:00Z' WHERE name = $1")
			.bind(&tenant)
			.execute(self.federation.store.pool.fixture_driver())
			.await
			.unwrap();
		let context = self.context(person, &tenant).await;
		(tenant, context)
	}

	async fn make_personal(&self, tenant: &str) {
		sqlx::query("UPDATE tenants SET personal = true WHERE name = $1")
			.bind(tenant)
			.execute(self.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	}

	fn control(&self) -> &aidash_server::database::native::Pool {
		&self.federation.store.control_pool
	}

	async fn subscribe(&self, account: Uuid, tier: &str, status: Status) {
		store::record_subscription(
			self.control(),
			account,
			&Subscription {
				tier: tier.into(),
				status,
				grace_ends_at: (status == Status::PastDue).then(|| Utc::now() + Duration::days(14)),
			},
		)
		.await
		.unwrap();
	}

	async fn account(&self, owner: &Person, tier: Option<&str>) -> Uuid {
		let (status, created) = self
			.call(
				owner,
				"POST",
				"/api/billing/accounts",
				None,
				json!({"label":"Payer"}),
			)
			.await;
		assert_eq!(status, 201, "{created}");
		let account: Uuid = created["id"].as_str().unwrap().parse().unwrap();
		if let Some(tier) = tier {
			self.subscribe(account, tier, Status::Active).await;
		}
		account
	}

	/// Both consents to covering `tenant` by `account`.
	async fn attach(
		&self,
		administrator: &Person,
		context: &str,
		owner: &Person,
		tenant: &str,
		account: Uuid,
	) -> ((u16, Value), (u16, Value)) {
		let first = self
			.call(
				administrator,
				"POST",
				&format!("/api/tenants/{tenant}/billing/attachment"),
				Some(context),
				json!({"account": account}),
			)
			.await;
		let second = self
			.call(
				owner,
				"POST",
				&format!("/api/billing/accounts/{account}/attachments"),
				None,
				json!({"tenant": tenant}),
			)
			.await;
		(first, second)
	}

	async fn scalar<T>(&self, sql: &str) -> T
	where
		T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
	{
		sqlx::query_scalar(sql)
			.fetch_one(self.federation.store.pool.fixture_driver())
			.await
			.unwrap()
	}

	async fn coverage(&self, tenant: &str) -> Vec<Coverage> {
		let rows: Vec<CoverageRow> = sqlx::query_as("SELECT id, account_id, starts_at, ends_at, consented_by_administrator, consented_by_owner FROM billing_coverage_periods WHERE tenant = $1 ORDER BY starts_at")
			.bind(tenant)
			.fetch_all(self.federation.store.pool.fixture_driver())
			.await
			.unwrap();
		rows.into_iter()
			.map(
				|(
					id,
					account_id,
					starts_at,
					ends_at,
					consented_by_administrator,
					consented_by_owner,
				)| Coverage {
					id,
					account_id,
					starts_at,
					ends_at,
					consented_by_administrator,
					consented_by_owner,
				},
			)
			.collect()
	}

	async fn historical_coverage(
		&self,
		tenant: &str,
		account: Uuid,
		identity: Uuid,
		start: DateTime<Utc>,
		end: Option<DateTime<Utc>>,
	) -> Uuid {
		let id = Uuid::new_v4();
		let pool = self.federation.store.pool.fixture_driver();
		sqlx::query(
			"UPDATE billing_accounts SET created_at = '1970-01-01T00:00:00Z' WHERE id = $1",
		)
		.bind(account)
		.execute(pool)
		.await
		.unwrap();
		sqlx::query("INSERT INTO billing_coverage_periods (id, tenant, account_id, starts_at, ends_at, consented_by_administrator, consented_by_owner) VALUES ($1,$2,$3,$4,$5,$6,$6)")
			.bind(id).bind(tenant).bind(account).bind(start).bind(end).bind(identity).execute(pool).await.unwrap();
		id
	}

	async fn historical_deletion(
		&self,
		tenant: &str,
		start: DateTime<Utc>,
		end: Option<DateTime<Utc>>,
	) {
		sqlx::query("INSERT INTO tenant_deletions (id,tenant,deleted_at,deleted_by_kind,purge_after,restored_at,restored_by_kind,purge_failures) VALUES ($1,$2,$3,'deployment_administrator',$4,$5,$6,0)")
			.bind(Uuid::new_v4()).bind(tenant).bind(start).bind(start+Duration::days(30)).bind(end)
			.bind(end.map(|_| "deployment_administrator")).execute(self.federation.store.pool.fixture_driver()).await.unwrap();
	}

	/// Queues a Run in `tenant`, which the admission gate may refuse.
	async fn queue_run(&self, tenant: &str) -> aidash_server::Result<()> {
		let f = &self.federation;
		let scope = TenancyScope::tenant(tenant).unwrap();
		let name = aidash_domain::tenancy::TenantName::new(tenant).unwrap();
		let workspace = f
			.store
			.create_workspace(&scope, "owner", "Work", "Admission")
			.await?;
		let task = f
			.store
			.create_task(
				&scope,
				workspace.id,
				&aidash_server::domain::NewTask {
					title: "Task".into(),
					description: "Admission".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"human",
				None,
			)
			.await?;
		let suffix = Uuid::new_v4().simple().to_string();
		let model = format!("model-{suffix}");
		let agent = format!("agent-{suffix}");
		let mut registered = None;
		for (kind, id, config) in [
			(
				"model",
				model.clone(),
				json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:1/v1","context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}),
			),
			(
				"agent",
				agent,
				json!({"schema_version":1,"model":{"id":model,"version":"1.0.0"},"instructions":"Fixture","bindings":[],"remove_default":[]}),
			),
		] {
			registered = Some(
				f.registry
					.register(
						&name,
						serde_json::from_value(json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"Fixture"},"schema":{},"config":config})).unwrap(),
					)
					.await?,
			);
		}
		let agent = registered.unwrap();
		let owner = aidash_server::domain::qualified_agent(
			&f.store.deployment.node(workspace.node.clone()),
			&agent.id,
			&agent.version,
		);
		f.store
			.claim(&scope, task.id, task.revision, &owner, &agent)
			.await
			.map(|_| ())
	}

	async fn finish(self) {
		drop(self.app);
		common::cleanup(self.federation, &self.url, &self.database).await;
	}
}

#[rstest::rstest]
#[tokio::test]
async fn node_creation_uses_the_enterprise_adapter_and_current_subscription(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Arrange
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_of(&alice).await;
	d.make_personal(&tenant).await;
	let path = format!("/api/tenants/{tenant}/nodes");
	// Act / Assert: Free covers its existing default Node only.
	let (status, refused) = d
		.call(
			&alice,
			"POST",
			&path,
			Some(&context),
			json!({"label":"Second"}),
		)
		.await;
	assert_eq!(status, 403, "{refused}");
	assert_eq!(refused["error"], "node_limit_reached");
	let account = d.account(&alice, Some("team")).await;
	let (_, (status, attached)) = d.attach(&alice, &context, &alice, &tenant, account).await;
	assert_eq!(status, 200, "{attached}");
	assert_eq!(attached["state"], "attached");
	let (status, created) = d
		.call(
			&alice,
			"POST",
			&path,
			Some(&context),
			json!({"label":"Second"}),
		)
		.await;
	assert_eq!(status, 201, "{created}");
	// Billing reads the real count before allowing a Tier change as well.
	let mut billing = aidash_enterprise_server::billing::billing(&d.federation).unwrap();
	let mut limits = sandbox_defaults();
	limits["tiers"]["team"]["nodes"] = json!(1);
	billing.settings = Arc::new(Settings::from_value(limits).unwrap());
	let changed = billing.change_tier(alice.identity, account, "team").await;
	assert!(
		matches!(changed, Err(aidash_server::Error::Conflict(ref code)) if code == "tier_node_limit"),
		"{changed:?}"
	);
	// The adapter reads a later Subscription state, rather than caching the Tier.
	d.subscribe(account, "team", Status::Suspended).await;
	let (status, refused) = d
		.call(
			&alice,
			"POST",
			&path,
			Some(&context),
			json!({"label":"Third"}),
		)
		.await;
	assert_eq!(status, 409, "{refused}");
	assert_eq!(refused["error"], "billing_suspended");
	d.finish().await;
}

fn refusal(result: aidash_server::Result<()>) -> String {
	match result {
		Err(aidash_server::Error::Refused(code)) => code,
		other => panic!("expected a refusal, got {other:?}"),
	}
}

fn at(offset: Duration) -> DateTime<Utc> {
	Utc::now() + offset
}

#[rstest::rstest]
#[tokio::test]
async fn pod_time_is_metered_once_and_stops_when_a_tenant_deletion_is_recorded(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let meter = Meter {
		pool: d.control().clone(),
	};
	let start = at(-Duration::hours(10));
	let operation = Uuid::new_v4();
	let event = |operation, from: DateTime<Utc>, to: DateTime<Utc>| PodTime {
		allocation: operation,
		receipt_digest: format!("{:x}", Sha256::digest(operation.as_bytes())),
		tenant: tenant.clone(),
		started_at: from,
		ended_at: to,
	};
	meter
		.pod_time(event(operation, start, start + Duration::seconds(90)))
		.await
		.unwrap();
	// The runner's report arrives again after a retry: the same Pod.
	meter
		.pod_time(event(operation, start, start + Duration::seconds(90)))
		.await
		.unwrap();
	let metered: i64 = d
		.scalar("SELECT COALESCE(sum(millis), 0)::bigint FROM billing_pod_usage")
		.await;
	assert_eq!(metered, 90_000);
	// A Deletion recorded 4 hours ago: a Pod that ran across it is metered
	// only until it, and one that began after it is not metered at all.
	let deleted_at = at(-Duration::hours(4));
	sqlx::query(
		"INSERT INTO tenant_deletions (id, tenant, deleted_at, deleted_by_kind, purge_after, purge_failures) VALUES ($1, $2, $3, 'deployment_administrator', $4, 0)",
	)
	.bind(Uuid::new_v4())
	.bind(&tenant)
	.bind(deleted_at)
	.bind(at(Duration::days(29)))
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	let across = Uuid::new_v4();
	meter
		.pod_time(event(
			across,
			deleted_at - Duration::seconds(60),
			deleted_at + Duration::seconds(600),
		))
		.await
		.unwrap();
	meter
		.pod_time(event(
			Uuid::new_v4(),
			deleted_at + Duration::seconds(10),
			deleted_at + Duration::seconds(20),
		))
		.await
		.unwrap();
	let across_millis: i64 =
		sqlx::query_scalar("SELECT millis FROM billing_pod_usage WHERE allocation = $1")
			.bind(across)
			.fetch_one(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(across_millis, 60_000);
	let rows: i64 = d.scalar("SELECT count(*) FROM billing_pod_usage").await;
	assert_eq!(
		rows, 3,
		"even fully exempt allocations retain their raw receipt"
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn stored_bytes_are_sampled_daily_through_physical_purge_except_for_deleted_tenants(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob) = (d.person("alice").await, d.person("bob").await);
	let (kept, _) = d.tenant_of(&alice).await;
	let (deleted, _) = d.tenant_of(&bob).await;
	for (tenant, size) in [(&kept, 1000_i64), (&kept, 234), (&deleted, 77)] {
		sqlx::query(
			"INSERT INTO core_objects (id, tenant, kind, digest, size, created_at) VALUES ($1, $2, 'blob', 'digest', $3, clock_timestamp())",
		)
		.bind(Uuid::new_v4())
		.bind(tenant)
		.bind(size)
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	}
	sqlx::query(
		"INSERT INTO tenant_deletions (id, tenant, deleted_at, deleted_by_kind, purge_after, purge_failures) VALUES ($1, $2, $3, 'deployment_administrator', $4, 0)",
	)
	.bind(Uuid::new_v4())
	.bind(&deleted)
	.bind(at(-Duration::days(3) - Duration::hours(1)))
	.bind(at(Duration::days(26)))
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	// A page image is stored bytes too. It belongs to a Research fragment of a
	// Run, so the Run comes from a real claim and the source, snapshot and
	// fragment rows are the ones its foreign keys require. A Team Tenant with
	// no paid Subscription refuses a Run, so the creator's personal Tenant
	// runs on the Free Tier.
	d.make_personal(&kept).await;
	d.queue_run(&kept).await.unwrap();
	let pool = d.federation.store.pool.fixture_driver();
	let run: Uuid = sqlx::query_scalar("SELECT id FROM runs WHERE tenant = $1")
		.bind(&kept)
		.fetch_one(pool)
		.await
		.unwrap();
	let (source, snapshot, fragment) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
	let digest = format!("sha256:{}", "0".repeat(64));
	let mut seed = pool.begin().await.unwrap();
	sqlx::query(
		"INSERT INTO research_sources (id, tenant, run_id, url, title) VALUES ($1, $2, $3, 'https://example.com/report.pdf', 'Report')",
	)
	.bind(source)
	.bind(&kept)
	.bind(run)
	.execute(&mut *seed)
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO research_snapshots (id, tenant, run_id, source_id, requested_url, url, title, media_type, fetched_at, raw_digest, text_digest, completeness, lines, text_bytes) VALUES ($1, $2, $3, $4, 'https://example.com/report.pdf', 'https://example.com/report.pdf', 'Report', 'application/pdf', clock_timestamp(), $5, $5, 'complete', '[]'::jsonb, 0)",
	)
	.bind(snapshot)
	.bind(&kept)
	.bind(run)
	.bind(source)
	.bind(&digest)
	.execute(&mut *seed)
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO research_fragments (id, tenant, run_id, snapshot_id, text, line_start, line_end, pages, image_page, image_media_type, image_width, image_height, image_digest) VALUES ($1, $2, $3, $4, '', 0, 0, '[1]'::jsonb, 1, 'image/png', 1, 1, $5)",
	)
	.bind(fragment)
	.bind(&kept)
	.bind(run)
	.bind(snapshot)
	.bind(&digest)
	.execute(&mut *seed)
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO research_page_images (fragment_id, tenant, run_id, bytes, created_at) VALUES ($1, $2, $3, $4, clock_timestamp())",
	)
	.bind(fragment)
	.bind(&kept)
	.bind(run)
	.bind(vec![0_u8; 4000])
	.execute(&mut *seed)
	.await
	.unwrap();
	seed.commit().await.unwrap();
	let now = Utc::now() - Duration::days(3);
	// The sampler includes bootstrap/Personal Tenants as well as the two named
	// fixtures. Their birth must also precede this simulated historical pass.
	sqlx::query("UPDATE tenants SET created_at = LEAST(created_at, $1)")
		.bind(now - Duration::days(1))
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	let samples =
		aidash_server::apps::execution::services::storage_usage::samples(&d.federation.store, now)
			.await
			.unwrap();
	let bytes: Vec<_> = samples
		.iter()
		.map(|sample| (sample.tenant.clone(), sample.bytes))
		.collect();
	assert!(bytes.contains(&(kept.clone(), 1234 + 4000)), "{bytes:?}");
	assert!(bytes.contains(&(deleted.clone(), 77)), "{bytes:?}");
	let meter = Meter {
		pool: d.control().clone(),
	};
	meter.storage_sampled(samples.clone()).await.unwrap();
	// A second replica's pass the same day records nothing new.
	meter
		.storage_sampled(
			samples
				.iter()
				.map(|sample| StorageSample {
					bytes: 1,
					..sample.clone()
				})
				.collect(),
		)
		.await
		.unwrap();
	let stored: Vec<(String, i64)> = sqlx::query_as(
		"SELECT tenant, bytes FROM billing_storage_samples WHERE tenant IN ($1, $2) AND status <> 'exempt'",
	)
	.bind(&kept)
	.bind(&deleted)
	.fetch_all(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		stored,
		vec![(kept.clone(), 1234 + 4000)],
		"the deleted Tenant is not metered"
	);
	// Removing one source must leave the other physically retained bytes metered.
	sqlx::query("DELETE FROM core_objects WHERE tenant = $1")
		.bind(&kept)
		.execute(pool)
		.await
		.unwrap();
	let tomorrow = now + Duration::days(1);
	meter
		.storage_sampled(
			aidash_server::apps::execution::services::storage_usage::samples(
				&d.federation.store,
				tomorrow,
			)
			.await
			.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(
		store::latest_storage_bytes(d.control(), &kept)
			.await
			.unwrap(),
		4000
	);

	// Once the last physical file is purged, a later daily sample replaces the
	// nonzero latest usage with an explicit zero for the still-active Tenant.
	// Source deletion cascades through snapshots, fragments and page images,
	// leaving no source row from which the sampler could infer this Tenant.
	sqlx::query("DELETE FROM research_sources WHERE tenant = $1")
		.bind(&kept)
		.execute(pool)
		.await
		.unwrap();
	let after_purge = tomorrow + Duration::days(1);
	// A whole-Tenant purge keeps its root tombstone, but must not resume billing.
	sqlx::query("DELETE FROM core_objects WHERE tenant = $1")
		.bind(&deleted)
		.execute(pool)
		.await
		.unwrap();
	sqlx::query("UPDATE tenant_deletions SET purged_at = $1 WHERE tenant = $2")
		.bind(after_purge)
		.bind(&deleted)
		.execute(pool)
		.await
		.unwrap();
	meter
		.storage_sampled(
			aidash_server::apps::execution::services::storage_usage::samples(
				&d.federation.store,
				after_purge,
			)
			.await
			.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(
		store::latest_storage_bytes(d.control(), &kept)
			.await
			.unwrap(),
		0,
		"physical purge must not leave the previous nonzero sample as latest usage"
	);
	let days: Vec<(chrono::NaiveDate, i64)> = sqlx::query_as(
		"SELECT sampled_on, bytes FROM billing_storage_samples WHERE tenant = $1 ORDER BY sampled_on",
	)
	.bind(&kept)
	.fetch_all(pool)
	.await
	.unwrap();
	assert_eq!(
		days,
		vec![
			(now.date_naive(), 5234),
			(tomorrow.date_naive(), 4000),
			(after_purge.date_naive(), 0)
		]
	);
	let deleted_samples: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM billing_storage_samples WHERE tenant = $1 AND status <> 'exempt'",
	)
	.bind(&deleted)
	.fetch_one(pool)
	.await
	.unwrap();
	assert_eq!(
		deleted_samples, 0,
		"Tenant deletion remains exempt after physical purge leaves only its root"
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn records_older_than_the_retention_period_are_swept_and_the_rest_survive(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let meter = Meter {
		pool: d.control().clone(),
	};
	let now = Utc::now();
	let old = cutoff(now, 7) - Duration::days(1);
	let recent = cutoff(now, 7) + Duration::days(1);
	for (operation, ended) in [(Uuid::new_v4(), old), (Uuid::new_v4(), recent)] {
		meter
			.pod_time(PodTime {
				allocation: operation,
				receipt_digest: format!("{:x}", Sha256::digest(operation.as_bytes())),
				tenant: tenant.clone(),
				started_at: ended - Duration::seconds(5),
				ended_at: ended,
			})
			.await
			.unwrap();
	}
	for sampled_at in [old, recent] {
		meter
			.storage_sampled(vec![StorageSample {
				tenant: tenant.clone(),
				sampled_at,
				bytes: 5,
			}])
			.await
			.unwrap();
	}
	// A Tenant purged before the cutoff takes its attachment with it.
	let account = d.account(&alice, Some("team")).await;
	sqlx::query(
		"INSERT INTO billing_attachments (tenant, account_id, attached_at, consented_by_administrator, consented_by_owner) VALUES ($1, $2, clock_timestamp(), $3, $3)",
	)
	.bind(&tenant)
	.bind(account)
	.bind(alice.identity)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO tenant_deletions (id, tenant, deleted_at, deleted_by_kind, purge_after, purged_at, purge_failures) VALUES ($1, $2, $3, 'deployment_administrator', $4, $5, 0)",
	)
	.bind(Uuid::new_v4())
	.bind(&tenant)
	.bind(old - Duration::days(40))
	.bind(old - Duration::days(10))
	.bind(old - Duration::days(5))
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	store::sweep(d.control(), cutoff(now, 7), now)
		.await
		.unwrap();
	let pods: i64 = d.scalar("SELECT count(*) FROM billing_pod_usage").await;
	let samples: i64 = d
		.scalar("SELECT count(*) FROM billing_storage_samples")
		.await;
	let attachments: i64 = d.scalar("SELECT count(*) FROM billing_attachments").await;
	assert_eq!((pods, samples, attachments), (1, 1, 0));
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn accounts_and_subscriptions_past_the_retention_period_are_swept_and_the_rest_survive(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let now = Utc::now();
	let old = cutoff(now, 7) - Duration::days(1);
	let recent = cutoff(now, 7) + Duration::days(1);
	let created_long_ago = old - Duration::days(30);
	let ended_long_ago = d.account(&alice, None).await;
	let ended_recently = d.account(&alice, None).await;
	let never_subscribed = d.account(&alice, None).await;
	let attached = d.account(&alice, None).await;
	let consenting = d.account(&alice, None).await;
	d.subscribe(ended_long_ago, "team", Status::Canceled).await;
	d.subscribe(ended_recently, "team", Status::Canceled).await;
	let pool = d.federation.store.pool.fixture_driver();
	for (account, created, ended) in [
		(ended_long_ago, created_long_ago, Some(old)),
		(ended_recently, created_long_ago, Some(recent)),
		(never_subscribed, old, None),
		(attached, old, None),
		(consenting, old, None),
	] {
		sqlx::query("UPDATE billing_accounts SET created_at = $2 WHERE id = $1")
			.bind(account)
			.bind(created)
			.execute(pool)
			.await
			.unwrap();
		if let Some(ended) = ended {
			sqlx::query("UPDATE billing_subscriptions SET updated_at = $2 WHERE account_id = $1")
				.bind(account)
				.bind(ended)
				.execute(pool)
				.await
				.unwrap();
		}
	}
	sqlx::query(
		"INSERT INTO billing_attachments (tenant, account_id, attached_at, consented_by_administrator, consented_by_owner) VALUES ($1, $2, clock_timestamp(), $3, $3)",
	)
	.bind(&tenant)
	.bind(attached)
	.bind(alice.identity)
	.execute(pool)
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO billing_attachment_consents (id, tenant, account_id, party, identity_id, created_at, expires_at) VALUES ($1, $2, $3, 'tenant_administrator', $4, clock_timestamp(), clock_timestamp() + interval '7 days')",
	)
	.bind(Uuid::new_v4())
	.bind(&tenant)
	.bind(consenting)
	.bind(alice.identity)
	.execute(pool)
	.await
	.unwrap();
	store::sweep(d.control(), cutoff(now, 7), now)
		.await
		.unwrap();
	let mut accounts: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM billing_accounts")
		.fetch_all(pool)
		.await
		.unwrap();
	accounts.sort();
	let mut expected = vec![ended_recently, attached, consenting];
	expected.sort();
	assert_eq!(accounts, expected);
	let subscriptions: Vec<Uuid> =
		sqlx::query_scalar("SELECT account_id FROM billing_subscriptions")
			.fetch_all(pool)
			.await
			.unwrap();
	assert_eq!(subscriptions, vec![ended_recently]);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_is_attached_only_with_both_consents_and_within_its_tiers_limits(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob, carol) = (
		d.person("alice").await,
		d.person("bob").await,
		d.person("carol").await,
	);
	let (tenant, context) = d.tenant_of(&alice).await;
	let account = d.account(&bob, None).await;
	// The account's owner alone does not attach, and a Tenant Administrator
	// alone does not either.
	let ((status, first), (refused, second)) =
		d.attach(&alice, &context, &bob, &tenant, account).await;
	assert_eq!(
		(status, &first["state"]),
		(200, &json!("awaiting_other_party"))
	);
	assert_eq!(refused, 409, "no active paid Subscription yet: {second}");
	assert_eq!(second["error"], "subscription_required");
	d.subscribe(account, "team", Status::Active).await;
	let (status, attached) = d
		.call(
			&bob,
			"POST",
			&format!("/api/billing/accounts/{account}/attachments"),
			None,
			json!({"tenant": tenant}),
		)
		.await;
	assert_eq!(
		(status, &attached["state"]),
		(200, &json!("attached")),
		"{attached}"
	);
	let (status, accounts) = d
		.call(&bob, "GET", "/api/billing/accounts", None, Value::Null)
		.await;
	assert_eq!(status, 200, "{accounts}");
	assert_eq!(accounts[0]["tenants"], json!([tenant]));
	assert_eq!(accounts[0]["tier"], "team");
	// Someone who does not own the account cannot consent for it, and a
	// person without a Mapping that allows `billing.attach` cannot consent
	// for the Tenant.
	let (status, _) = d
		.call(
			&carol,
			"POST",
			&format!("/api/billing/accounts/{account}/attachments"),
			None,
			json!({"tenant": tenant}),
		)
		.await;
	assert_eq!(status, 404);
	let (status, _) = d
		.call(
			&carol,
			"POST",
			&format!("/api/tenants/{tenant}/billing/attachment"),
			Some(&context),
			json!({"account": account}),
		)
		.await;
	assert_eq!(status, 403);
	// Moving needs both consents again, from the receiving account's owner.
	let other = d.account(&carol, Some("team")).await;
	let ((_, waiting), (_, moved)) = d.attach(&alice, &context, &carol, &tenant, other).await;
	assert_eq!(waiting["state"], "awaiting_other_party");
	assert_eq!(moved["state"], "attached", "{moved}");
	let (_, first_account) = d
		.call(&bob, "GET", "/api/billing/accounts", None, Value::Null)
		.await;
	assert_eq!(first_account[0]["tenants"], json!([]));
	// The Tier's Tenant limit (5) holds.
	for _ in 0..4 {
		let (extra, extra_context) = d.tenant_of(&alice).await;
		let (_, (_, result)) = d
			.attach(&alice, &extra_context, &carol, &extra, other)
			.await;
		assert_eq!(result["state"], "attached", "{result}");
	}
	let (sixth, sixth_context) = d.tenant_of(&alice).await;
	let (_, (status, result)) = d
		.attach(&alice, &sixth_context, &carol, &sixth, other)
		.await;
	assert_eq!(
		(status, &result["error"]),
		(409, &json!("tier_tenant_limit")),
		"{result}"
	);
	// A suspended Subscription takes no more Tenants.
	d.subscribe(other, "team", Status::Suspended).await;
	let (last, last_context) = d.tenant_of(&alice).await;
	let (_, (status, result)) = d.attach(&alice, &last_context, &carol, &last, other).await;
	assert_eq!(
		(status, &result["error"]),
		(409, &json!("subscription_required")),
		"{result}"
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_tier_change_that_would_exceed_a_limit_is_refused_nodes_included(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob) = (d.person("alice").await, d.person("bob").await);
	let mut value = sandbox_defaults();
	value["tiers"]["small"] =
		json!({"tenants": 2, "nodes": 2, "pod_seconds": 10, "storage_bytes": 10});
	let tiers = Arc::new(Settings::from_value(value).unwrap());
	let payments = Arc::new(FakePayments::default());
	let (first, _) = d.tenant_of(&alice).await;
	let (second, _) = d.tenant_of(&alice).await;
	let (third, _) = d.tenant_of(&alice).await;
	let account = store::create_account(d.control(), bob.identity, "Payer")
		.await
		.unwrap()
		.id;
	payments.set(
		account,
		Subscription {
			tier: "team".into(),
			status: Status::Active,
			grace_ends_at: None,
		},
	);
	let billing = |nodes: FixedNodes| Billing {
		pool: d.control().clone(),
		settings: tiers.clone(),
		payments: payments.clone(),
		nodes: Arc::new(nodes),
	};
	let service = billing(FixedNodes::default());
	for tenant in [&first, &second, &third] {
		let waiting = service
			.consent(Party::TenantAdministrator, alice.identity, tenant, account)
			.await
			.unwrap();
		assert_eq!(waiting, Consented::AwaitingOtherParty);
		let attached = service
			.consent(Party::AccountOwner, bob.identity, tenant, account)
			.await
			.unwrap();
		assert_eq!(attached, Consented::Attached);
	}
	let code = |result: aidash_server::Result<()>| match result {
		Err(aidash_server::Error::Conflict(code)) => code,
		other => panic!("expected a refusal, got {other:?}"),
	};
	// Three Tenants do not fit the small Tier's two.
	assert_eq!(
		code(service.change_tier(bob.identity, account, "small").await),
		"tier_tenant_limit"
	);
	// With the Node limit: a Tenant with 3 Nodes does not fit 2.
	let nodes = FixedNodes([(first.clone(), 3)].into());
	sqlx::query("DELETE FROM billing_attachments WHERE tenant = ANY($1)")
		.bind(vec![second.clone(), third.clone()])
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		code(
			billing(nodes)
				.change_tier(bob.identity, account, "small")
				.await
		),
		"tier_node_limit"
	);
	assert!(
		payments.changes().is_empty(),
		"a refused change never reaches the processor"
	);
	// Without either problem the change goes to the processor port.
	service
		.change_tier(bob.identity, account, "small")
		.await
		.unwrap();
	assert_eq!(payments.changes(), vec![(account, "small".to_owned())]);
	// Another person cannot change the account; Free is not a Subscription's Tier.
	assert!(
		service
			.change_tier(alice.identity, account, "team")
			.await
			.is_err()
	);
	assert!(
		service
			.change_tier(bob.identity, account, "free")
			.await
			.is_err()
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn new_runs_follow_the_free_tier_a_paid_subscription_and_suspension(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob) = (d.person("alice").await, d.person("bob").await);
	let (tenant, context) = d.tenant_of(&alice).await;
	// A shared Tenant, with no Billing Account, may not start Runs.
	assert_eq!(refusal(d.queue_run(&tenant).await), "billing_required");
	// A Personal Tenant whose only Mapping is its creator's is Free.
	d.make_personal(&tenant).await;
	d.queue_run(&tenant).await.unwrap();
	// The Free Tier's included usage is also its ceiling.
	let meter = Meter {
		pool: d.control().clone(),
	};
	let ended = Utc::now();
	meter
		.pod_time(PodTime {
			allocation: Uuid::new_v4(),
			receipt_digest: "a".repeat(64),
			tenant: tenant.clone(),
			started_at: ended - Duration::seconds(3600),
			ended_at: ended,
		})
		.await
		.unwrap();
	assert_eq!(refusal(d.queue_run(&tenant).await), "free_usage_exhausted");
	// An attached account with an active paid Subscription covers it.
	let account = d.account(&bob, Some("team")).await;
	let (_, (_, attached)) = d.attach(&alice, &context, &bob, &tenant, account).await;
	assert_eq!(attached["state"], "attached");
	d.queue_run(&tenant).await.unwrap();
	// Past due, within the grace period, still starts Runs.
	d.subscribe(account, "team", Status::PastDue).await;
	d.queue_run(&tenant).await.unwrap();
	// After the grace period the account is suspended, whether or not the
	// processor's report has arrived.
	store::record_subscription(
		d.control(),
		account,
		&Subscription {
			tier: "team".into(),
			status: Status::PastDue,
			grace_ends_at: Some(at(-Duration::hours(1))),
		},
	)
	.await
	.unwrap();
	assert_eq!(refusal(d.queue_run(&tenant).await), "billing_suspended");
	// Reading keeps working, and nothing deletes the Tenant.
	let (status, session) = d
		.call(&alice, "GET", "/auth/session", None, Value::Null)
		.await;
	assert_eq!(status, 200, "{session}");
	let (status, _) = d
		.call(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/administration"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200);
	let deleted: i64 = d.scalar("SELECT count(*) FROM tenant_deletions").await;
	assert_eq!(deleted, 0);
	// Resumption: the processor reports the account active again.
	d.subscribe(account, "team", Status::Active).await;
	d.queue_run(&tenant).await.unwrap();
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn admission_decides_from_held_state_and_dedicated_admits_everything(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let account = d.account(&alice, Some("team")).await;
	sqlx::query(
		"INSERT INTO billing_attachments (tenant, account_id, attached_at, consented_by_administrator, consented_by_owner) VALUES ($1, $2, clock_timestamp(), $3, $3)",
	)
	.bind(&tenant)
	.bind(account)
	.bind(alice.identity)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	/// A payment state whose processor is down: it answers from what Aidash
	/// holds and any call that would need the processor fails the test.
	struct ProcessorDown(Subscription);
	#[async_trait::async_trait]
	impl PaymentState for ProcessorDown {
		async fn subscription(&self, _: Uuid) -> aidash_server::Result<Option<Subscription>> {
			Ok(Some(self.0.clone()))
		}
		async fn change_tier(&self, _: Uuid, _: &str) -> aidash_server::Result<()> {
			panic!("admission never asks the processor");
		}
	}
	let gate = |offering, subscription| Gate {
		pool: d.control().clone(),
		settings: settings(),
		payments: Arc::new(ProcessorDown(subscription)),
		offering,
	};
	let active = Subscription {
		tier: "team".into(),
		status: Status::Active,
		grace_ends_at: None,
	};
	let suspended = Subscription {
		status: Status::Suspended,
		..active.clone()
	};
	for work in [Work::Run, Work::SandboxedTool] {
		assert_eq!(
			gate(Offering::Cloud, active.clone())
				.admit(&tenant, work)
				.await
				.unwrap(),
			Verdict::Admit
		);
		assert_eq!(
			gate(Offering::Cloud, suspended.clone())
				.admit(&tenant, work)
				.await
				.unwrap(),
			Verdict::refuse("billing_suspended")
		);
		// Dedicated meters and reports usage, and never refuses work.
		assert_eq!(
			gate(Offering::Dedicated, suspended.clone())
				.admit(&tenant, work)
				.await
				.unwrap(),
			Verdict::Admit
		);
	}
	// The Mirror reads Aidash's own rows, which is all admission needs.
	assert!(
		Mirror {
			pool: d.control().clone()
		}
		.subscription(account)
		.await
		.unwrap()
		.is_some()
	);
	// An unknown Tenant is for every other boundary to refuse.
	assert_eq!(
		gate(Offering::Cloud, suspended)
			.admit("no-such-tenant", Work::Run)
			.await
			.unwrap(),
		Verdict::Admit
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_second_member_joins_a_personal_tenant_only_after_it_has_an_attached_subscription(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob, carol) = (
		d.person("alice").await,
		d.person("bob").await,
		d.person("carol").await,
	);
	let (tenant, context) = d.tenant_of(&alice).await;
	d.make_personal(&tenant).await;
	let (status, invitation) = d
		.call(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&context),
			json!({}),
		)
		.await;
	assert_eq!(status, 201, "{invitation}");
	let link = json!({"tenant": tenant, "secret": invitation["secret"]});
	let (status, request) = d
		.call(&bob, "POST", "/auth/invitations/redeem", None, link)
		.await;
	assert_eq!(status, 200, "{request}");
	let (_, pending) = d
		.call(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/registrations"),
			Some(&context),
			Value::Null,
		)
		.await;
	let id = pending[0]["id"].as_str().unwrap().to_owned();
	let approve = format!("/api/tenants/{tenant}/registrations/{id}/approve");
	let body = json!({"subject":"bob","groups":["members"]});
	let (status, refused) = d
		.call(&alice, "POST", &approve, Some(&context), body.clone())
		.await;
	assert_eq!(
		(status, &refused["error"]),
		(402, &json!("billing_required")),
		"{refused}"
	);
	let account = d.account(&carol, Some("team")).await;
	let (_, (_, attached)) = d.attach(&alice, &context, &carol, &tenant, account).await;
	assert_eq!(attached["state"], "attached", "{attached}");
	let (status, approved) = d.call(&alice, "POST", &approve, Some(&context), body).await;
	assert_eq!(status, 200, "{approved}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_deployment_administrator_reads_usage_as_json_and_csv(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let meter = Meter {
		pool: d.control().clone(),
	};
	let ended = Utc::now();
	meter
		.pod_time(PodTime {
			allocation: Uuid::new_v4(),
			receipt_digest: "a".repeat(64),
			tenant: tenant.clone(),
			started_at: ended - Duration::milliseconds(90_500),
			ended_at: ended,
		})
		.await
		.unwrap();
	meter
		.storage_sampled(vec![StorageSample {
			tenant: tenant.clone(),
			sampled_at: ended,
			bytes: 4096,
		}])
		.await
		.unwrap();
	let today = ended.date_naive();
	let query = format!("from={today}&to={today}");
	let (status, report, ..) = d
		.send(
			"GET",
			&format!("/api/usage?{query}"),
			None,
			None,
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{report}");
	assert_eq!(
		report["rows"],
		json!([{"tenant": tenant, "date": today.to_string(), "pod_seconds": 91, "storage_bytes": 4096}])
	);
	let (status, _, headers, csv) = d
		.send(
			"GET",
			&format!("/api/usage.csv?{query}"),
			None,
			None,
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{csv}");
	assert!(
		headers["content-type"]
			.to_str()
			.unwrap()
			.starts_with("text/csv")
	);
	assert_eq!(
		csv,
		format!("tenant,date,pod_seconds,storage_bytes\r\n{tenant},{today},91,4096\r\n")
	);
	// Only a Deployment Administrator reads it, never a Tenant's member.
	let context = d.context(&alice, &tenant).await;
	let (status, _) = d
		.call(
			&alice,
			"GET",
			&format!("/api/usage?{query}"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 403);
	let (status, ..) = d
		.send(
			"GET",
			"/api/usage?from=2026-01-05&to=2026-01-01",
			None,
			None,
			Value::Null,
		)
		.await;
	assert_eq!(status, 400);
	d.finish().await;
}

/// Pauses the processor before publishing the new locally readable Tier.
struct DelayedTier {
	pool: aidash_server::database::native::Pool,
	entered: tokio::sync::Notify,
	release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl PaymentState for DelayedTier {
	async fn subscription(&self, account: Uuid) -> aidash_server::Result<Option<Subscription>> {
		store::subscription(&self.pool, account).await
	}
	async fn change_tier(&self, account: Uuid, tier: &str) -> aidash_server::Result<()> {
		self.entered.notify_one();
		self.release.notified().await;
		store::record_subscription(
			&self.pool,
			account,
			&Subscription {
				tier: tier.into(),
				status: Status::Active,
				grace_ends_at: None,
			},
		)
		.await
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_tier_downgrade_and_node_creation_share_the_capacity_lock(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let mut d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_of(&alice).await;
	let account = d.account(&alice, Some("team")).await;
	let (_, (status, body)) = d.attach(&alice, &context, &alice, &tenant, account).await;
	assert_eq!(status, 200, "{body}");
	let mut tiers = sandbox_defaults();
	tiers["tiers"]["small"] =
		json!({"tenants":5,"nodes":1,"pod_seconds":3600,"storage_bytes":1073741824});
	let tiers = Arc::new(Settings::from_value(tiers).unwrap());
	let payments = Arc::new(DelayedTier {
		pool: d.control().clone(),
		entered: Default::default(),
		release: Default::default(),
	});
	d.federation.config.tier_limits = Arc::new(Gate {
		pool: d.control().clone(),
		settings: tiers.clone(),
		payments: payments.clone(),
		offering: Offering::Cloud,
	});
	d.app.context.set_singleton(d.federation.clone());
	let billing = Billing {
		pool: d.control().clone(),
		settings: tiers,
		payments: payments.clone(),
		nodes: Arc::new(aidash_enterprise_server::billing::nodes::StoredNodes(
			d.federation.store.pool.clone(),
		)),
	};
	{
		let change = billing.change_tier(alice.identity, account, "small");
		tokio::pin!(change);
		tokio::select! {
			_ = payments.entered.notified() => {},
			result = &mut change => panic!("processor did not wait: {result:?}"),
		}
		let path = format!("/api/tenants/{tenant}/nodes");
		let create = d.call(
			&alice,
			"POST",
			&path,
			Some(&context),
			json!({"label":"Concurrent"}),
		);
		tokio::pin!(create);
		// The request must reach the database capacity lock, rather than being
		// allowed to commit under the old paid Tier while the processor waits.
		let blocked = async {
			loop {
				let waiting: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event = 'advisory' AND query LIKE '%pg_advisory_xact_lock%'")
					.fetch_one(d.federation.store.pool.fixture_driver()).await.unwrap();
				if waiting > 0 {
					break;
				}
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		};
		let attempted = tokio::select! {
			result = &mut create => Some(result),
			result = tokio::time::timeout(std::time::Duration::from_secs(5), blocked) => { result.expect("Node creation must wait at the capacity lock"); None },
		};
		payments.release.notify_one();
		change.await.unwrap();
		let (status, body) = match attempted {
			Some(result) => result,
			None => create.await,
		};
		assert_eq!(status, 403, "creation must see the downgraded Tier: {body}");
		assert_eq!(body["error"], "node_limit_reached");
		assert_eq!(
			store::subscription(d.control(), account)
				.await
				.unwrap()
				.unwrap()
				.tier,
			"small"
		);
	}

	d.finish().await;
}

/// Replaying an authorized attachment must not consume a different pending move,
/// rewrite the consent authors, or bypass the HTTP authority/deletion checks.
#[rstest::rstest]
#[tokio::test]
async fn coverage_same_account_is_an_authorized_noop(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob, carol) = (
		d.person("alice").await,
		d.person("bob").await,
		d.person("carol").await,
	);
	let (tenant, context) = d.tenant_of(&alice).await;
	let account = d.account(&bob, Some("team")).await;
	let other = d.account(&carol, Some("team")).await;
	assert_eq!(
		d.attach(&alice, &context, &bob, &tenant, account).await.1.1["state"],
		"attached"
	);
	let pool = d.federation.store.pool.fixture_driver();
	let before: Value =
		sqlx::query_scalar("SELECT to_jsonb(a) FROM billing_attachments a WHERE tenant = $1")
			.bind(&tenant)
			.fetch_one(pool)
			.await
			.unwrap();
	let (_, pending) = d
		.call(
			&carol,
			"POST",
			&format!("/api/billing/accounts/{other}/attachments"),
			None,
			json!({"tenant":tenant}),
		)
		.await;
	assert_eq!(pending["state"], "awaiting_other_party");
	// Even a suspended account remains an idempotent attachment; this admits no
	// new capacity and must not ask for or record either party's consent again.
	d.subscribe(account, "team", Status::Suspended).await;
	for (person, path, selector, body) in [
		(
			&alice,
			format!("/api/tenants/{tenant}/billing/attachment"),
			Some(context.as_str()),
			json!({"account":account}),
		),
		(
			&bob,
			format!("/api/billing/accounts/{account}/attachments"),
			None,
			json!({"tenant":tenant}),
		),
	] {
		let (status, result) = d.call(person, "POST", &path, selector, body).await;
		assert_eq!(
			(status, &result["state"]),
			(200, &json!("attached")),
			"{result}"
		);
	}
	let after: Value =
		sqlx::query_scalar("SELECT to_jsonb(a) FROM billing_attachments a WHERE tenant = $1")
			.bind(&tenant)
			.fetch_one(pool)
			.await
			.unwrap();
	assert_eq!(after, before);
	let periods = d.coverage(&tenant).await;
	assert_eq!(periods.len(), 1);
	assert_eq!(
		periods[0].starts_at,
		DateTime::parse_from_rfc3339(before["attached_at"].as_str().unwrap())
			.unwrap()
			.with_timezone(&Utc)
	);
	assert_eq!(
		(
			periods[0].consented_by_administrator,
			periods[0].consented_by_owner
		),
		(alice.identity, bob.identity)
	);
	let remaining: Vec<Uuid> =
		sqlx::query_scalar("SELECT account_id FROM billing_attachment_consents WHERE tenant = $1")
			.bind(&tenant)
			.fetch_all(pool)
			.await
			.unwrap();
	assert_eq!(remaining, vec![other]);
	let (status, _) = d
		.call(
			&carol,
			"POST",
			&format!("/api/billing/accounts/{account}/attachments"),
			None,
			json!({"tenant":tenant}),
		)
		.await;
	assert_eq!(status, 404);
	sqlx::query(
		"UPDATE dashboard_mappings SET enabled = false WHERE tenant = $1 AND identity_id = $2",
	)
	.bind(&tenant)
	.bind(alice.identity)
	.execute(pool)
	.await
	.unwrap();
	let (status, _) = d
		.call(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/billing/attachment"),
			Some(&context),
			json!({"account":account}),
		)
		.await;
	assert_eq!(status, 403);
	sqlx::query("INSERT INTO tenant_deletions (id, tenant, deleted_at, deleted_by_kind, purge_after, purge_failures) VALUES ($1, $2, clock_timestamp(), 'deployment_administrator', clock_timestamp() + interval '30 days', 0)")
        .bind(Uuid::new_v4()).bind(&tenant).execute(pool).await.unwrap();
	let (status, _) = d
		.call(
			&bob,
			"POST",
			&format!("/api/billing/accounts/{account}/attachments"),
			None,
			json!({"tenant":tenant}),
		)
		.await;
	assert_eq!(status, 404);
	assert_eq!(d.coverage(&tenant).await, periods);
	d.finish().await;
}

/// Either consent order produces one interval; a later move ends it exactly
/// where the next starts, atomically with the admission projection. A delayed
/// allocation retains each account's frozen usage across that actual move.
#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn coverage_moves_share_one_half_open_boundary(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[case] owner_first: bool,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob) = (d.person("alice").await, d.person("bob").await);
	let (tenant, context) = d.tenant_of(&alice).await;
	let first = d.account(&bob, Some("team")).await;
	let second = d.account(&bob, Some("team")).await;
	let mut allocation_started_at = None;
	for account in [first, second] {
		let admin_path = format!("/api/tenants/{tenant}/billing/attachment");
		let owner_path = format!("/api/billing/accounts/{account}/attachments");
		let admin = d.call(
			&alice,
			"POST",
			&admin_path,
			Some(&context),
			json!({"account":account}),
		);
		let owner = d.call(&bob, "POST", &owner_path, None, json!({"tenant":tenant}));
		let (pending, attached) = if owner_first {
			(owner.await, admin.await)
		} else {
			(admin.await, owner.await)
		};
		assert_eq!(pending.1["state"], "awaiting_other_party");
		assert_eq!(
			(attached.0, &attached.1["state"]),
			(200, &json!("attached")),
			"{:?}",
			attached
		);
		if account == first {
			// Observe the fixture allocation's start before requesting the move;
			// its lifetime is not manufactured from the eventual move boundary.
			allocation_started_at =
				Some(d.scalar::<DateTime<Utc>>("SELECT clock_timestamp()").await);
		}
	}
	let allocation_ended_at: DateTime<Utc> = d.scalar("SELECT clock_timestamp()").await;
	let periods = d.coverage(&tenant).await;
	assert_eq!(periods.len(), 2);
	assert_eq!(
		(periods[0].account_id, periods[1].account_id),
		(first, second)
	);
	assert_eq!(periods[0].ends_at, Some(periods[1].starts_at));
	assert!(periods[0].starts_at < periods[1].starts_at);
	assert_eq!(periods[1].ends_at, None);
	for period in &periods {
		assert_eq!(
			(period.consented_by_administrator, period.consented_by_owner),
			(alice.identity, bob.identity)
		);
	}
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_attachment_consents")
			.await,
		0
	);
	// Deliver only after the move has committed. Independent DB observations
	// bracket its persisted boundary without sleeps or invented historical spans.
	let event = receipt(&tenant, allocation_started_at.unwrap(), allocation_ended_at);
	let boundary = periods[1].starts_at;
	assert!(periods[0].starts_at <= event.started_at);
	assert!(event.started_at < boundary && boundary < event.ended_at);
	let pool = d.federation.store.pool.fixture_driver();
	let mut frozen = None;
	for _ in 0..2 {
		// Use the installed production sink, including an identical delayed retry.
		d.federation
			.store
			.usage
			.pod_time(event.clone())
			.await
			.unwrap();
		let raw: Value =
			sqlx::query_scalar("SELECT to_jsonb(p) FROM billing_pod_usage p WHERE allocation = $1")
				.bind(event.allocation)
				.fetch_one(pool)
				.await
				.unwrap();
		assert_eq!(
			raw["started_ns"],
			event.started_at.timestamp_nanos_opt().unwrap()
		);
		assert_eq!(
			raw["ended_ns"],
			event.ended_at.timestamp_nanos_opt().unwrap()
		);
		assert_eq!(raw["receipt_digest"], event.receipt_digest);
		let segments: Vec<Value> = sqlx::query_scalar(
			"SELECT to_jsonb(s) FROM billing_usage_segments s WHERE allocation = $1 ORDER BY starts_ns",
		)
		.bind(event.allocation)
		.fetch_all(pool)
		.await
		.unwrap();
		assert_eq!(segments.len(), 2);
		for (index, (period, start, end)) in [
			(&periods[0], event.started_at, boundary),
			(&periods[1], boundary, event.ended_at),
		]
		.into_iter()
		.enumerate()
		{
			let nanos = (end - start).num_nanoseconds().unwrap().to_string();
			assert_eq!(segments[index]["status"], "covered");
			assert_eq!(segments[index]["account_id"], json!(period.account_id));
			assert_eq!(segments[index]["coverage_id"], json!(period.id));
			let path = format!(
				"/api/billing/accounts/{}/usage?starts_at={}&ends_at={}",
				period.account_id,
				event
					.started_at
					.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
				event
					.ended_at
					.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
			);
			let (status, usage) = d.call(&bob, "GET", &path, None, Value::Null).await;
			assert_eq!(status, 200, "{usage}");
			assert_eq!(usage["account"], json!(period.account_id));
			assert_eq!(usage["pod_nanos"], nanos);
			assert_eq!(
				usage["segments"],
				json!([{
					"allocation": event.allocation,
					"tenant": tenant,
					"coverage_id": period.id,
					"starts_at": start,
					"ends_at": end,
					"nanos": nanos,
				}]),
			);
		}
		if let Some(expected) = &frozen {
			assert_eq!(
				&(raw, segments),
				expected,
				"retry must retain the original raw and frozen facts"
			);
		} else {
			frozen = Some((raw, segments));
		}
	}
	let current: (Uuid, DateTime<Utc>) =
		sqlx::query_as("SELECT account_id, attached_at FROM billing_attachments WHERE tenant = $1")
			.bind(&tenant)
			.fetch_one(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(current, (second, periods[1].starts_at));
	assert_eq!(d.coverage(&tenant).await, periods);
	// Storage rejects a zero-length closed period and two open periods even
	// when a writer bypasses the service. Ordinary adjacent spans are above.
	for (start, end) in [
		(periods[1].starts_at, Some(periods[1].starts_at)),
		(periods[1].starts_at + Duration::seconds(1), None),
	] {
		assert!(sqlx::query("INSERT INTO billing_coverage_periods (id, tenant, account_id, starts_at, ends_at, consented_by_administrator, consented_by_owner) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(Uuid::new_v4()).bind(&tenant).bind(first).bind(start).bind(end).bind(alice.identity).bind(bob.identity)
            .execute(d.federation.store.pool.fixture_driver()).await.is_err());
	}
	d.finish().await;
}

/// Hold the production capacity lock, observe the actual HTTP waiter, then
/// compare the committed boundary with the DB clock just before releasing it.
#[rstest::rstest]
#[tokio::test]
async fn coverage_uses_the_database_clock_after_capacity_lock_wait(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob) = (d.person("alice").await, d.person("bob").await);
	let (tenant, context) = d.tenant_of(&alice).await;
	let account = d.account(&bob, Some("team")).await;
	let (_, waiting) = d
		.call(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/billing/attachment"),
			Some(&context),
			json!({"account":account}),
		)
		.await;
	assert_eq!(waiting["state"], "awaiting_other_party");
	let mut held = d.control().begin(&TenancyScope::NoTenant).await.unwrap();
	aidash_server::authorization::services::nodes::lock_capacity(&mut *held, &tenant)
		.await
		.unwrap();
	let pid: i32 = aidash_server::database::native::query_scalar("SELECT pg_backend_pid()")
		.scalar_one(&mut *held)
		.await
		.unwrap();
	let path = format!("/api/billing/accounts/{account}/attachments");
	{
		let request = d.call(&bob, "POST", &path, None, json!({"tenant":tenant}));
		tokio::pin!(request);
		tokio::select! {
			_ = common::wait_for_blocked(d.control(), pid, 1) => {},
			result = &mut request => panic!("attachment did not wait: {result:?}"),
		}
		let released_at: DateTime<Utc> = d.scalar("SELECT clock_timestamp()").await;
		held.commit().await.unwrap();
		let (status, attached) = request.await;
		assert_eq!(
			(status, &attached["state"]),
			(200, &json!("attached")),
			"{attached}"
		);
		let periods = d.coverage(&tenant).await;
		assert_eq!(periods.len(), 1);
		assert!(
			periods[0].starts_at >= released_at,
			"boundary {:?} predates lock release {released_at}",
			periods[0].starts_at
		);
	}
	d.finish().await;
}

/// Two target-account locks do not serialize moves from one Tenant. The shared
/// Tenant capacity lock must consume the competing consents exactly once.
#[rstest::rstest]
#[tokio::test]
async fn coverage_concurrent_moves_cannot_overlap_or_reuse_consumed_consents(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob) = (d.person("alice").await, d.person("bob").await);
	let (tenant, context) = d.tenant_of(&alice).await;
	let initial = d.account(&bob, Some("team")).await;
	assert_eq!(
		d.attach(&alice, &context, &bob, &tenant, initial).await.1.1["state"],
		"attached"
	);
	let first = d.account(&bob, Some("team")).await;
	let second = d.account(&bob, Some("team")).await;
	for account in [first, second] {
		let (_, result) = d
			.call(
				&alice,
				"POST",
				&format!("/api/tenants/{tenant}/billing/attachment"),
				Some(&context),
				json!({"account":account}),
			)
			.await;
		assert_eq!(result["state"], "awaiting_other_party");
	}
	let mut held = d.control().begin(&TenancyScope::NoTenant).await.unwrap();
	aidash_server::authorization::services::nodes::lock_capacity(&mut *held, &tenant)
		.await
		.unwrap();
	let pid: i32 = aidash_server::database::native::query_scalar("SELECT pg_backend_pid()")
		.scalar_one(&mut *held)
		.await
		.unwrap();
	let path1 = format!("/api/billing/accounts/{first}/attachments");
	let path2 = format!("/api/billing/accounts/{second}/attachments");
	{
		let moves = async {
			tokio::join!(
				d.call(&bob, "POST", &path1, None, json!({"tenant":tenant})),
				d.call(&bob, "POST", &path2, None, json!({"tenant":tenant}))
			)
		};
		tokio::pin!(moves);
		tokio::select! {
			_ = common::wait_for_blocked(d.control(), pid, 2) => {},
			result = &mut moves => panic!("moves did not serialize: {result:?}"),
		}
		held.commit().await.unwrap();
		let (a, b) = moves.await;
		assert_eq!((a.0, b.0), (200, 200), "{a:?} {b:?}");
		let mut states = vec![
			a.1["state"].as_str().unwrap(),
			b.1["state"].as_str().unwrap(),
		];
		states.sort();
		assert_eq!(states, ["attached", "awaiting_other_party"]);
		let periods = d.coverage(&tenant).await;
		assert_eq!(periods.len(), 2);
		assert_eq!(periods[0].ends_at, Some(periods[1].starts_at));
		assert_eq!(periods[1].ends_at, None);
		let current: Uuid =
			sqlx::query_scalar("SELECT account_id FROM billing_attachments WHERE tenant = $1")
				.bind(&tenant)
				.fetch_one(d.federation.store.pool.fixture_driver())
				.await
				.unwrap();
		assert_eq!(periods[1].account_id, current);
	}
	d.finish().await;
}

/// A closed period can expire only after its retained usage does; the account
/// reference and open coverage both survive Tenant purge and audit expiry.
#[rstest::rstest]
#[tokio::test]
async fn coverage_retention_keeps_referenced_history_and_accounts_after_tenant_purge(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let now = Utc::now();
	let boundary = cutoff(now, 7);
	let expired = d.account(&alice, None).await;
	let pod_referenced = d.account(&alice, None).await;
	let recent = d.account(&alice, None).await;
	let open = d.account(&alice, None).await;
	let pool = d.federation.store.pool.fixture_driver();
	for (account, start, end) in [
		(
			expired,
			boundary - Duration::days(10),
			Some(boundary - Duration::days(9)),
		),
		(
			pod_referenced,
			boundary - Duration::days(8),
			Some(boundary - Duration::days(7)),
		),
		(
			recent,
			boundary + Duration::days(1),
			Some(boundary + Duration::days(2)),
		),
		(open, boundary + Duration::days(3), None),
	] {
		sqlx::query("UPDATE billing_accounts SET created_at = $2 WHERE id = $1")
			.bind(account)
			.bind(boundary - Duration::days(20))
			.execute(pool)
			.await
			.unwrap();
		sqlx::query("INSERT INTO billing_coverage_periods (id, tenant, account_id, starts_at, ends_at, consented_by_administrator, consented_by_owner) VALUES ($1,$2,$3,$4,$5,$6,$6)")
            .bind(Uuid::new_v4()).bind(&tenant).bind(account).bind(start).bind(end).bind(alice.identity).execute(pool).await.unwrap();
	}
	// This report starts in an expired period but ends within retention. The
	// raw lifetime still needs that historical account to be attributable.
	Meter {
		pool: d.control().clone(),
	}
	.pod_time(PodTime {
		allocation: Uuid::new_v4(),
		receipt_digest: "a".repeat(64),
		tenant: tenant.clone(),
		started_at: boundary - Duration::days(8) + Duration::hours(1),
		ended_at: boundary + Duration::hours(1),
	})
	.await
	.unwrap();
	Meter {
		pool: d.control().clone(),
	}
	.storage_sampled(vec![StorageSample {
		tenant: tenant.clone(),
		sampled_at: boundary + Duration::days(1) + Duration::hours(1),
		bytes: 5,
	}])
	.await
	.unwrap();
	sqlx::query("INSERT INTO tenant_deletions (id, tenant, deleted_at, deleted_by_kind, purge_after, purged_at, audit_erased_at, purge_failures) VALUES ($1,$2,$3,'deployment_administrator',$4,$5,$6,0)")
        .bind(Uuid::new_v4()).bind(&tenant).bind(now - Duration::days(400)).bind(now - Duration::days(370)).bind(now - Duration::days(369)).bind(now).execute(pool).await.unwrap();
	store::sweep(d.control(), boundary, now).await.unwrap();
	let periods = d.coverage(&tenant).await;
	assert_eq!(
		periods.iter().map(|p| p.account_id).collect::<Vec<_>>(),
		[pod_referenced, recent, open]
	);
	let mut accounts: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM billing_accounts")
		.fetch_all(pool)
		.await
		.unwrap();
	accounts.sort();
	let mut expected = vec![pod_referenced, recent, open];
	expected.sort();
	assert_eq!(accounts, expected);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM tenant_deletions")
			.await,
		1
	);
	// The next cutoff expires the Pod/sample and every closed period. An open
	// period remains unknown-ended, even after purge; no detach is invented.
	store::sweep(d.control(), boundary + Duration::days(4), now)
		.await
		.unwrap();
	let periods = d.coverage(&tenant).await;
	assert_eq!(periods.len(), 1);
	assert_eq!((periods[0].account_id, periods[0].ends_at), (open, None));
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_accounts")
			.await,
		1
	);
	d.finish().await;
}

/// The target-account lock serializes attachments of different Tenants, so a
/// one-slot Tier cannot commit two concurrent receiving attachments.
#[rstest::rstest]
#[tokio::test]
async fn coverage_concurrent_attachments_respect_account_capacity(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob) = (d.person("alice").await, d.person("bob").await);
	let (first, _) = d.tenant_of(&alice).await;
	let (second, _) = d.tenant_of(&alice).await;
	let account = d.account(&bob, Some("team")).await;
	let mut tiers = sandbox_defaults();
	tiers["tiers"]["team"]["tenants"] = json!(1);
	let billing = Billing {
		pool: d.control().clone(),
		settings: Arc::new(Settings::from_value(tiers).unwrap()),
		payments: Arc::new(Mirror {
			pool: d.control().clone(),
		}),
		nodes: Arc::new(FixedNodes::default()),
	};
	for tenant in [&first, &second] {
		assert_eq!(
			billing
				.consent(Party::TenantAdministrator, alice.identity, tenant, account)
				.await
				.unwrap(),
			Consented::AwaitingOtherParty
		);
	}
	let mut held = d.control().begin(&TenancyScope::NoTenant).await.unwrap();
	store::lock_account(&mut *held, account)
		.await
		.unwrap()
		.unwrap();
	let pid: i32 = aidash_server::database::native::query_scalar("SELECT pg_backend_pid()")
		.scalar_one(&mut *held)
		.await
		.unwrap();
	{
		let requests = async {
			tokio::join!(
				billing.consent(Party::AccountOwner, bob.identity, &first, account),
				billing.consent(Party::AccountOwner, bob.identity, &second, account)
			)
		};
		tokio::pin!(requests);
		tokio::select! {
			_ = common::wait_for_blocked(d.control(), pid, 2) => {},
			result = &mut requests => panic!("requests did not reach the account lock: {result:?}"),
		}
		held.commit().await.unwrap();
		let results = requests.await;
		let mut attached = 0;
		for result in [results.0, results.1] {
			match result {
				Ok(Consented::Attached) => attached += 1,
				Err(aidash_server::Error::Conflict(code)) if code == "tier_tenant_limit" => {}
				result => panic!("unexpected capacity outcome: {result:?}"),
			}
		}
		assert_eq!(attached, 1);
		assert_eq!(
			d.scalar::<i64>("SELECT count(*) FROM billing_coverage_periods")
				.await,
			1
		);
		assert_eq!(
			d.scalar::<i64>("SELECT count(*) FROM billing_attachments")
				.await,
			1
		);
	}
	d.finish().await;
}

/// Stored live capacity is the common boundary for deletion, restoration,
/// account attachment and Tier changes; retained Node rows do not reserve slots.
#[rstest::rstest]
#[tokio::test]
async fn deleted_nodes_release_billing_capacity_and_restore_serializes_with_downgrade(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	use aidash_enterprise_server::billing::nodes::{NodeCount, StoredNodes};
	let mut d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_of(&alice).await;
	let account = d.account(&alice, Some("team")).await;
	let (_, (status, body)) = d.attach(&alice, &context, &alice, &tenant, account).await;
	assert_eq!(status, 200, "{body}");
	let mut tiers = sandbox_defaults();
	tiers["tiers"]["small"] =
		json!({"tenants":5,"nodes":1,"pod_seconds":3600,"storage_bytes":1073741824});
	let tiers = Arc::new(Settings::from_value(tiers).unwrap());
	let payments = Arc::new(FakePayments::default());
	payments.set(
		account,
		Subscription {
			tier: "team".into(),
			status: Status::Active,
			grace_ends_at: None,
		},
	);
	d.federation.config.tier_limits = Arc::new(Gate {
		pool: d.control().clone(),
		settings: tiers.clone(),
		payments: payments.clone(),
		offering: Offering::Cloud,
	});
	d.app.context.set_singleton(d.federation.clone());
	let counts = Arc::new(StoredNodes(d.federation.store.pool.clone()));
	let billing = Billing {
		pool: d.control().clone(),
		settings: tiers,
		payments: payments.clone(),
		nodes: counts.clone(),
	};
	let path = format!("/api/tenants/{tenant}/nodes");
	let (status, created) = d
		.call(
			&alice,
			"POST",
			&path,
			Some(&context),
			json!({"label":"Recoverable"}),
		)
		.await;
	assert_eq!(status, 201, "{created}");
	let node = created["name"].as_str().unwrap();
	assert_eq!(counts.nodes(&tenant).await.unwrap(), 2);
	{
		let delete_path = format!("{path}/{node}/deletion");
		let wait_for_capacity_contenders = async || {
			tokio::time::timeout(std::time::Duration::from_secs(10), async {
			loop {
				let waiting: i64 = d.scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory' AND query LIKE '%pg_advisory_xact_lock%'").await;
				if waiting >= 2 { break; }
				tokio::time::sleep(std::time::Duration::from_millis(10)).await;
			}
		}).await.expect("both real lifecycle and billing requests must wait on capacity");
		};
		let mut held = d.control().begin(&TenancyScope::NoTenant).await.unwrap();
		aidash_server::authorization::services::nodes::lock_capacity(&mut *held, &tenant)
			.await
			.unwrap();
		let operations = async {
			tokio::join!(
				d.call(
					&alice,
					"POST",
					&delete_path,
					Some(&context),
					json!({"confirmation":node})
				),
				billing.change_tier(alice.identity, account, "small"),
			)
		};
		tokio::pin!(operations);
		tokio::select! {
			_ = wait_for_capacity_contenders() => {},
			result = &mut operations => panic!("operations escaped the held capacity lock: {result:?}"),
		}
		held.commit().await.unwrap();
		let (deleted, downgrade) = operations.await;
		assert_eq!(deleted.0, 200, "{deleted:?}");
		if let Err(error) = downgrade {
			assert!(
				matches!(&error, aidash_server::Error::Conflict(code) if code == "tier_node_limit"),
				"{error:?}"
			);
			billing
				.change_tier(alice.identity, account, "small")
				.await
				.unwrap();
		}
		assert_eq!(counts.nodes(&tenant).await.unwrap(), 1);
		let restore_path = format!("{path}/{node}/restore");
		let (status, body) = d
			.call(&alice, "POST", &restore_path, Some(&context), Value::Null)
			.await;
		assert_eq!(
			status, 409,
			"the downgraded current limit prevents restore: {body}"
		);
		assert_eq!(body["error"], "node_limit_reached");

		// Attachment uses the same live count, despite the retained deleted row.
		let smaller = d.account(&alice, None).await;
		payments.set(
			smaller,
			Subscription {
				tier: "small".into(),
				status: Status::Active,
				grace_ends_at: None,
			},
		);
		assert!(matches!(
			billing
				.consent(Party::TenantAdministrator, alice.identity, &tenant, smaller)
				.await
				.unwrap(),
			Consented::AwaitingOtherParty
		));
		assert!(matches!(
			billing
				.consent(Party::AccountOwner, alice.identity, &tenant, smaller)
				.await
				.unwrap(),
			Consented::Attached
		));
		billing
			.change_tier(alice.identity, smaller, "team")
			.await
			.unwrap();
		let mut held = d.control().begin(&TenancyScope::NoTenant).await.unwrap();
		aidash_server::authorization::services::nodes::lock_capacity(&mut *held, &tenant)
			.await
			.unwrap();
		let operations = async {
			tokio::join!(
				d.call(&alice, "POST", &restore_path, Some(&context), Value::Null),
				billing.change_tier(alice.identity, smaller, "small"),
			)
		};
		tokio::pin!(operations);
		tokio::select! {
			_ = wait_for_capacity_contenders() => {},
			result = &mut operations => panic!("operations escaped the held capacity lock: {result:?}"),
		}
		held.commit().await.unwrap();
		let (restored, downgraded) = operations.await;
		let live = counts.nodes(&tenant).await.unwrap();
		let tier = payments.subscription(smaller).await.unwrap().unwrap().tier;
		match restored.0 {
			200 => {
				let error =
					downgraded.expect_err("restore and downgrade cannot both exceed capacity");
				assert!(
					matches!(&error, aidash_server::Error::Conflict(code) if code == "tier_node_limit"),
					"{error:?}"
				);
				assert_eq!(live, 2);
				assert_eq!(tier, "team");
			}
			409 => {
				assert_eq!(restored.1["error"], "node_limit_reached");
				downgraded.unwrap();
				assert_eq!(live, 1);
				assert_eq!(tier, "small");
			}
			status => panic!("unexpected restore status {status}: {}", restored.1),
		}
	}
	d.finish().await;
}

fn receipt(tenant: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> PodTime {
	let allocation = Uuid::new_v4();
	PodTime {
		allocation,
		receipt_digest: format!("{:x}", Sha256::digest(allocation.as_bytes())),
		tenant: tenant.into(),
		started_at: start,
		ended_at: end,
	}
}

fn history_start() -> DateTime<Utc> {
	"2026-09-30T23:59:59Z".parse().unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn attribution_freezes_exact_history_and_clips_authorized_account_periods(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let (alice, bob) = (d.person("alice").await, d.person("bob").await);
	let (tenant, _) = d.tenant_of(&alice).await;
	let (first, second) = (d.account(&alice, None).await, d.account(&alice, None).await);
	let b = history_start();
	let split = b + Duration::seconds(1);
	let a = d
		.historical_coverage(
			&tenant,
			first,
			alice.identity,
			b + Duration::microseconds(1),
			Some(split),
		)
		.await;
	let z = d
		.historical_coverage(
			&tenant,
			second,
			alice.identity,
			split + Duration::microseconds(2),
			None,
		)
		.await;
	// Their union, not their sum, exempts exactly1,500,000ns.
	d.historical_deletion(
		&tenant,
		b + Duration::microseconds(500),
		Some(b + Duration::microseconds(1500)),
	)
	.await;
	d.historical_deletion(
		&tenant,
		b + Duration::microseconds(1000),
		Some(b + Duration::microseconds(2000)),
	)
	.await;
	let event = receipt(
		&tenant,
		b + Duration::nanoseconds(1),
		b + Duration::seconds(2) + Duration::nanoseconds(999),
	);
	let meter = Meter {
		pool: d.control().clone(),
	};
	meter.pod_time(event.clone()).await.unwrap();
	let pool = d.federation.store.pool.fixture_driver();
	let raw: (i64, i64, String) = sqlx::query_as(
		"SELECT started_ns,ended_ns,receipt_digest FROM billing_pod_usage WHERE allocation=$1",
	)
	.bind(event.allocation)
	.fetch_one(pool)
	.await
	.unwrap();
	assert_eq!(
		raw,
		(
			event.started_at.timestamp_nanos_opt().unwrap(),
			event.ended_at.timestamp_nanos_opt().unwrap(),
			event.receipt_digest.clone()
		)
	);
	let totals: Vec<(String,i64)> = sqlx::query_as("SELECT status,sum(ends_ns-starts_ns)::bigint FROM billing_usage_segments WHERE allocation=$1 GROUP BY status ORDER BY status").bind(event.allocation).fetch_all(pool).await.unwrap();
	assert_eq!(
		totals,
		vec![
			("covered".into(), 1_998_497_999),
			("exempt".into(), 1_500_000),
			("uncovered".into(), 2_999)
		]
	);
	let partition: Vec<(i64, i64)> = sqlx::query_as(
		"SELECT starts_ns,ends_ns FROM billing_usage_segments WHERE allocation=$1 ORDER BY starts_ns",
	)
	.bind(event.allocation)
	.fetch_all(pool)
	.await
	.unwrap();
	assert_eq!(partition[0].0, raw.0);
	assert_eq!(partition.last().unwrap().1, raw.1);
	assert!(partition.iter().all(|(start, end)| start < end));
	assert!(partition.windows(2).all(|pair| pair[0].1 == pair[1].0));
	let provenances: Vec<(Uuid,Uuid)> = sqlx::query_as("SELECT DISTINCT coverage_id,account_id FROM billing_usage_segments WHERE allocation=$1 AND status='covered' ORDER BY account_id").bind(event.allocation).fetch_all(pool).await.unwrap();
	let mut expected = vec![(a, first), (z, second)];
	expected.sort_by_key(|p| p.1);
	assert_eq!(provenances, expected);
	let query = |account, from: DateTime<Utc>, to: DateTime<Utc>| {
		format!(
			"/api/billing/accounts/{account}/usage?starts_at={}&ends_at={}",
			from.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
			to.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
		)
	};
	let path = query(first, b, split);
	let (status, usage) = d.call(&alice, "GET", &path, None, Value::Null).await;
	assert_eq!(status, 200, "{usage}");
	assert_eq!(usage["pod_nanos"], "998499000");
	let (_, tiny) = d
		.call(
			&alice,
			"GET",
			&query(
				first,
				b + Duration::nanoseconds(999),
				b + Duration::nanoseconds(1001),
			),
			None,
			Value::Null,
		)
		.await;
	assert_eq!(tiny["pod_nanos"], "1");
	let (_, right) = d
		.call(
			&alice,
			"GET",
			&query(first, b + Duration::nanoseconds(1001), split),
			None,
			Value::Null,
		)
		.await;
	assert_eq!(right["pod_nanos"], "998498999");
	let (_, later) = d
		.call(
			&alice,
			"GET",
			&query(second, split, b + Duration::seconds(3)),
			None,
			Value::Null,
		)
		.await;
	assert_eq!(later["pod_nanos"], "999998999");
	assert_eq!(d.call(&bob, "GET", &path, None, Value::Null).await.0, 404);
	assert_eq!(
		d.call(
			&alice,
			"GET",
			&query(first, split, split),
			None,
			Value::Null
		)
		.await
		.0,
		400
	);
	// A1ns allocation wholly inside a deletion remains an immutable raw fact.
	let exempt = receipt(
		&tenant,
		b + Duration::microseconds(600) + Duration::nanoseconds(1),
		b + Duration::microseconds(600) + Duration::nanoseconds(2),
	);
	meter.pod_time(exempt.clone()).await.unwrap();
	let exempt_ns: i64 = sqlx::query_scalar(
		"SELECT ends_ns-starts_ns FROM billing_usage_segments WHERE allocation=$1 AND status='exempt'",
	)
	.bind(exempt.allocation)
	.fetch_one(pool)
	.await
	.unwrap();
	assert_eq!(exempt_ns, 1);
	let frozen:Value = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(s) ORDER BY starts_ns) FROM billing_usage_segments s WHERE allocation=$1").bind(event.allocation).fetch_one(pool).await.unwrap();
	// Later lifecycle evidence must not recompute a committed receipt on retry.
	d.historical_deletion(
		&tenant,
		b + Duration::seconds(1),
		Some(b + Duration::seconds(3)),
	)
	.await;
	meter.pod_time(event.clone()).await.unwrap();
	let replayed:Value = sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(s) ORDER BY starts_ns) FROM billing_usage_segments s WHERE allocation=$1").bind(event.allocation).fetch_one(pool).await.unwrap();
	assert_eq!(replayed, frozen);
	for changed in [
		PodTime {
			receipt_digest: "b".repeat(64),
			..event.clone()
		},
		PodTime {
			ended_at: event.ended_at + Duration::nanoseconds(1),
			..event.clone()
		},
		PodTime {
			tenant: "missing-tenant".into(),
			..event.clone()
		},
	] {
		assert!(meter.pod_time(changed).await.is_err());
	}
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn attribution_rejects_invalid_or_contradictory_evidence_without_partial_facts(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let b = history_start();
	let meter = Meter {
		pool: d.control().clone(),
	};
	for event in [
		receipt(&tenant, b, b),
		receipt(&tenant, b, b - Duration::nanoseconds(1)),
		receipt(
			&tenant,
			Utc::now() + Duration::days(1),
			Utc::now() + Duration::days(2),
		),
		PodTime {
			receipt_digest: "A".repeat(64),
			..receipt(&tenant, b, b + Duration::seconds(1))
		},
		PodTime {
			allocation: Uuid::nil(),
			..receipt(&tenant, b, b + Duration::seconds(1))
		},
	] {
		assert!(meter.pod_time(event).await.is_err());
	}
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_pod_usage")
			.await,
		0
	);
	let a = d.account(&alice, None).await;
	d.historical_coverage(
		&tenant,
		a,
		alice.identity,
		b,
		Some(b + Duration::seconds(2)),
	)
	.await;
	d.historical_coverage(
		&tenant,
		a,
		alice.identity,
		b + Duration::seconds(1),
		Some(b + Duration::seconds(3)),
	)
	.await;
	assert!(
		meter
			.pod_time(receipt(&tenant, b, b + Duration::seconds(4)))
			.await
			.is_err()
	);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_pod_usage")
			.await,
		0
	);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_usage_segments")
			.await,
		0
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn attribution_samples_freeze_successor_zero_and_exempt_facts_across_replicas(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let (first, second) = (d.account(&alice, None).await, d.account(&alice, None).await);
	let b = history_start() + Duration::seconds(1);
	d.historical_coverage(
		&tenant,
		first,
		alice.identity,
		b - Duration::days(1),
		Some(b),
	)
	.await;
	let successor = d
		.historical_coverage(&tenant, second, alice.identity, b, None)
		.await;
	let meter = Meter {
		pool: d.control().clone(),
	};
	let samples = vec![StorageSample {
		tenant: tenant.clone(),
		sampled_at: b,
		bytes: 0,
	}];
	let (a, z) = tokio::join!(
		meter.storage_sampled(samples.clone()),
		meter.storage_sampled(samples)
	);
	a.unwrap();
	z.unwrap();
	meter
		.storage_sampled(vec![StorageSample {
			tenant: tenant.clone(),
			sampled_at: b + Duration::hours(1),
			bytes: 999,
		}])
		.await
		.unwrap();
	let row: (i64, Uuid, Uuid, String) = sqlx::query_as(
		"SELECT bytes,coverage_id,account_id,status FROM billing_storage_samples WHERE tenant=$1",
	)
	.bind(&tenant)
	.fetch_one(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(row, (0, successor, second, "covered".into()));
	d.historical_deletion(&tenant, b + Duration::days(1), None)
		.await;
	meter
		.storage_sampled(vec![StorageSample {
			tenant: tenant.clone(),
			sampled_at: b + Duration::days(1) + Duration::nanoseconds(1),
			bytes: 55,
		}])
		.await
		.unwrap();
	let precise: i64 =
		sqlx::query_scalar("SELECT sampled_ns FROM billing_storage_samples WHERE status='exempt'")
			.fetch_one(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(
		precise,
		(b + Duration::days(1) + Duration::nanoseconds(1))
			.timestamp_nanos_opt()
			.unwrap()
	);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_storage_samples WHERE status='exempt'")
			.await,
		1
	);
	assert_eq!(
		store::latest_storage_bytes(d.control(), &tenant)
			.await
			.unwrap(),
		0
	);
	let path = format!(
		"/api/billing/accounts/{second}/usage?starts_at={}&ends_at={}",
		b.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
		(b + Duration::days(2)).to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
	);
	let (status, report) = d.call(&alice, "GET", &path, None, Value::Null).await;
	assert_eq!(status, 200, "{report}");
	assert_eq!(report["samples"].as_array().unwrap().len(), 1);
	assert_eq!(report["samples"][0]["bytes"], 0);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn attribution_accepts_a_late_receipt_after_actual_completed_tenant_purge(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let account = d.account(&alice, None).await;
	let b = history_start();
	d.historical_coverage(&tenant, account, alice.identity, b, None)
		.await;
	let meter = Meter {
		pool: d.control().clone(),
	};
	meter
		.pod_time(receipt(&tenant, b, b + Duration::seconds(1)))
		.await
		.unwrap();
	meter
		.storage_sampled(vec![StorageSample {
			tenant: tenant.clone(),
			sampled_at: b,
			bytes: 7,
		}])
		.await
		.unwrap();
	let (status, result, ..) = d
		.send(
			"POST",
			&format!("/api/tenants/{tenant}/deletion"),
			None,
			None,
			json!({"confirmation":tenant}),
		)
		.await;
	assert_eq!(status, 200, "{result}");
	let pool = d.federation.store.pool.fixture_driver();
	sqlx::query("UPDATE tenant_deletions SET deleted_at=$2,purge_after=$3 WHERE tenant=$1")
		.bind(&tenant)
		.bind(b + Duration::days(1))
		.bind(b + Duration::days(2))
		.execute(pool)
		.await
		.unwrap();
	for deletion in aidash_server::authorization::services::tenant_purge::claim(&d.federation.store)
		.await
		.unwrap()
	{
		aidash_server::authorization::services::tenant_purge::purge_tenant(
			&d.federation.store,
			deletion,
		)
		.await
		.unwrap();
	}
	let purged: bool =
		sqlx::query_scalar("SELECT purged_at IS NOT NULL FROM tenant_deletions WHERE tenant=$1")
			.bind(&tenant)
			.fetch_one(pool)
			.await
			.unwrap();
	assert!(purged);
	let policies: i64 =
		sqlx::query_scalar("SELECT count(*) FROM authorization_bundles WHERE tenant=$1")
			.bind(&tenant)
			.fetch_one(pool)
			.await
			.unwrap();
	assert_eq!(policies, 0);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_pod_usage")
			.await,
		1
	);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_usage_segments")
			.await,
		1
	);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_storage_samples")
			.await,
		1
	);
	Meter {
		pool: d.control().clone(),
	}
	.pod_time(receipt(
		&tenant,
		b + Duration::seconds(1),
		b + Duration::seconds(3),
	))
	.await
	.unwrap();
	let attributed: Uuid =
		sqlx::query_scalar("SELECT account_id FROM billing_usage_segments WHERE status='covered'")
			.fetch_one(pool)
			.await
			.unwrap();
	assert_eq!(attributed, account);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_pod_usage")
			.await,
		2
	);
	assert_eq!(d.coverage(&tenant).await[0].ends_at, None);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn attribution_retention_waits_for_intake_and_rejects_unknown_old_history(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let (first, second) = (d.account(&alice, None).await, d.account(&alice, None).await);
	let b = history_start();
	d.historical_coverage(
		&tenant,
		first,
		alice.identity,
		b,
		Some(b + Duration::seconds(2)),
	)
	.await;
	d.historical_coverage(
		&tenant,
		second,
		alice.identity,
		b + Duration::seconds(2),
		None,
	)
	.await;
	let meter = Meter {
		pool: d.control().clone(),
	};
	let retained = receipt(&tenant, b, b + Duration::seconds(10));
	meter.pod_time(retained.clone()).await.unwrap();
	let mut held = d.control().begin(&TenancyScope::NoTenant).await.unwrap();
	aidash_server::authorization::services::nodes::lock_capacity(&mut *held, &tenant)
		.await
		.unwrap();
	let pid: i32 = aidash_server::database::native::query_scalar("SELECT pg_backend_pid()")
		.scalar_one(&mut *held)
		.await
		.unwrap();
	{
		let intake = meter.pod_time(receipt(
			&tenant,
			b + Duration::seconds(6),
			b + Duration::seconds(7),
		));
		tokio::pin!(intake);
		tokio::select! {
			_ = common::wait_for_blocked(d.control(),pid,1)=>{},
			result = &mut intake=>panic!("usage did not wait for history transition: {result:?}"),
		}
		// Intake already holds the real retention SHARE lock while waiting for
		// capacity. Actual sweep must now queue behind it, not prune its history.
		let sweep = store::sweep(d.control(), b + Duration::seconds(5), Utc::now());
		tokio::pin!(sweep);
		tokio::select! {
			_ = common::wait_for_blocked(d.control(),pid,2)=>{},
			result = &mut intake=>panic!("capacity holder was bypassed: {result:?}"),
			result = &mut sweep=>panic!("retention interleaved with intake: {result:?}"),
		}
		held.commit().await.unwrap();
		let (a, z) = tokio::join!(intake, sweep);
		a.unwrap();
		z.unwrap();
	}
	assert_eq!(
		d.coverage(&tenant).await.len(),
		2,
		"retained raw receipt still references old account coverage"
	);
	meter.pod_time(retained.clone()).await.unwrap();
	let rejected = meter
		.pod_time(receipt(
			&tenant,
			b + Duration::seconds(4),
			b + Duration::seconds(6),
		))
		.await
		.unwrap_err();
	assert!(rejected.to_string().contains("history"), "{rejected}");
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_pod_usage")
			.await,
		2
	);
	store::sweep(d.control(), b + Duration::seconds(11), Utc::now())
		.await
		.unwrap();
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_pod_usage")
			.await,
		0
	);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_usage_segments")
			.await,
		0
	);
	assert_eq!(d.coverage(&tenant).await.len(), 1);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_accounts")
			.await,
		1
	);
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_retention_state")
			.await,
		1
	);
	assert!(
		meter.pod_time(retained).await.is_err(),
		"expired identity must not be reconstructed from unknown old history"
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn attribution_intake_waits_for_the_actual_tenant_policy_lifecycle_lock(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Cloud::new(runtime).await;
	let alice = d.person("alice").await;
	let (tenant, _) = d.tenant_of(&alice).await;
	let pool = d.federation.store.pool.fixture_driver();
	// Tenant delete/restore exclusively lock this actual policy row. No fake
	// billing hook can stand in for participating in that lifecycle boundary.
	let mut held = pool.begin().await.unwrap();
	sqlx::query("SELECT tenant FROM authorization_bundles WHERE tenant=$1 FOR UPDATE")
		.bind(&tenant)
		.execute(&mut *held)
		.await
		.unwrap();
	let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
		.fetch_one(&mut *held)
		.await
		.unwrap();
	let meter = Meter {
		pool: d.control().clone(),
	};
	{
		let intake = meter.pod_time(receipt(
			&tenant,
			history_start(),
			history_start() + Duration::nanoseconds(1),
		));
		tokio::pin!(intake);
		tokio::select! {
			_ = common::wait_for_blocked(d.control(),pid,1)=>{},
			result = &mut intake=>panic!("usage bypassed Tenant deletion/restore serialization: {result:?}"),
		}
		held.commit().await.unwrap();
		intake.await.unwrap();
	}
	assert_eq!(
		d.scalar::<i64>("SELECT count(*) FROM billing_pod_usage")
			.await,
		1
	);
	d.finish().await;
}
