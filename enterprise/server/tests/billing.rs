//! The billing core on PostgreSQL: meters, Billing Accounts, Tiers, the rules
//! for who may exist on Cloud, Run admission and the usage report.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../server/src/apps/execution/tests/support/legacy.rs"]
mod common;
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
			public_origin: "http://127.0.0.1:8080".into(),
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
		.bind(Sha256::digest(name.as_bytes()).to_vec())
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
					.header("cookie", format!("aidash-session={}", person.name))
					.header("origin", "http://127.0.0.1:8080")
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
			&aidash_domain::nodes::NodeAddress::derived(f.store.deployment.as_str(), tenant),
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
		operation,
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
		sqlx::query_scalar("SELECT millis FROM billing_pod_usage WHERE operation = $1")
			.bind(across)
			.fetch_one(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(across_millis, 60_000);
	let rows: i64 = d.scalar("SELECT count(*) FROM billing_pod_usage").await;
	assert_eq!(rows, 2, "nothing is metered after the Deletion");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn stored_bytes_are_sampled_once_a_day_and_not_for_a_deleted_tenant(
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
	.bind(at(-Duration::hours(1)))
	.bind(at(Duration::days(29)))
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
	let now = Utc::now();
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
	let stored: Vec<(String, i64)> =
		sqlx::query_as("SELECT tenant, bytes FROM billing_storage_samples")
			.fetch_all(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(
		stored,
		vec![(kept, 1234 + 4000)],
		"the deleted Tenant is not metered"
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
				operation,
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
			operation: Uuid::new_v4(),
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
			operation: Uuid::new_v4(),
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
