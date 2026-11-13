//! Demo Reservations (#432, ADR 0079): the first External Identity that signs
//! in with a reserved verified email address is mapped, and loading maps an
//! identity that already exists, over PostgreSQL.
use super::*;
use aidash_domain::identity::dashboard::SignIn;
use aidash_domain::identity::tenant_administration::add_user_subject;
use aidash_server::apps::identity::models::DemoReservation;
use std::collections::BTreeSet;

const GOOGLE: &str = "https://accounts.google.com";

fn rules(personal_tenants: bool) -> TenancyRules {
	TenancyRules {
		personal_tenants,
		..TenancyRules::of(Offering::SelfHosted)
	}
}

impl Deployment {
	/// A Tenant a Deployment Administrator creates, with a user subject in the
	/// `members` group: what loading Demo data leaves before the people step.
	async fn demo_tenant(&self, label: &str, subject: &str) -> String {
		let (status, created) = self
			.as_deployment_administrator("POST", "/api/tenants", json!({"label":label}))
			.await;
		assert_eq!(status, 201, "{created}");
		let tenant = created["name"].as_str().unwrap().to_owned();
		let authorization = aidash_server::authorization::Authorization {
			pool: self.federation.store.pool.clone(),
		};
		let current = authorization.snapshot(&tenant).await.unwrap();
		let mut bundle = current.bundle.clone();
		add_user_subject(
			&mut bundle,
			subject,
			&BTreeSet::from(["members".to_owned()]),
		)
		.unwrap();
		authorization
			.replace(
				&tenant,
				current.revision,
				bundle,
				"deployment_administrator",
			)
			.await
			.unwrap();
		tenant
	}

	/// The people step for one person.
	async fn provide(&self, tenant: &str, email: &str, subject: &str) {
		DemoReservation::provide(
			&self.federation.store.pool,
			self.federation.config.dashboard_policy().as_ref(),
			tenant,
			email,
			subject,
		)
		.await
		.unwrap();
	}

	/// A sign-in of a new External Identity, as the dashboard admits it.
	async fn sign_in_as(&self, subject: &str, email: Option<&str>) -> Uuid {
		let account = aidash_server::authorization::sign_in::admit(
			&self.federation,
			&SignIn {
				issuer: GOOGLE.into(),
				subject: subject.into(),
				auth_time: chrono::Utc::now(),
				verified_email: email.map(str::to_owned),
				display_name: None,
			},
		)
		.await
		.unwrap();
		account.id
	}

	async fn mappings(&self, identity: Uuid, tenant: &str) -> i64 {
		self.count(&format!(
			"SELECT count(*) FROM dashboard_mappings WHERE identity_id = '{identity}' AND tenant = '{tenant}' AND enabled"
		))
		.await
	}

	/// Who spent the Reservation of `email` in `tenant`: `None` while unspent.
	async fn spent_by(&self, tenant: &str, email: &str) -> Option<Uuid> {
		sqlx::query_scalar(
			"SELECT spent_by FROM demo_reservations WHERE tenant = $1 AND email = $2",
		)
		.bind(tenant)
		.bind(email)
		.fetch_one(self.federation.store.pool.fixture_driver())
		.await
		.unwrap()
	}

	async fn personal_tenants(&self, identity: Uuid) -> i64 {
		self.count(&format!(
			"SELECT count(*) FROM personal_tenants WHERE identity_id = '{identity}'"
		))
		.await
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_reserved_email_signing_in_is_mapped_and_spends_the_reservation(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(true), true).await;
	let tenant = d.demo_tenant("Acme Research", "dana").await;
	d.provide(&tenant, "Dana@Example.com", "dana").await;
	assert_eq!(d.spent_by(&tenant, "dana@example.com").await, None);

	let identity = d.sign_in_as("dana-google", Some("DANA@example.com")).await;

	assert_eq!(d.mappings(identity, &tenant).await, 1);
	assert_eq!(
		d.spent_by(&tenant, "dana@example.com").await,
		Some(identity)
	);
	// The Reservation adds only the Mapping: the Personal Tenant follows the
	// Tenancy Rules as before.
	assert_eq!(d.personal_tenants(identity).await, 1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM dashboard_administration_history WHERE action = 'demo_reservation.fulfil' \
			 AND actor_kind = 'demo_data' AND identity_id = '{identity}' AND tenant = '{tenant}' AND subject = 'dana'"
		))
		.await,
		1
	);
	// A later sign-in of the same identity adds nothing.
	d.sign_in_as("dana-google", Some("dana@example.com")).await;
	assert_eq!(d.mappings(identity, &tenant).await, 1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM dashboard_mappings WHERE tenant = '{tenant}'"
		))
		.await,
		1
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_second_external_identity_with_the_same_email_gets_nothing(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), true).await;
	let tenant = d.demo_tenant("Acme Research", "dana").await;
	d.provide(&tenant, "dana@example.com", "dana").await;

	let first = d.sign_in_as("dana-google", Some("dana@example.com")).await;
	let second = d
		.sign_in_as("dana-password", Some("dana@example.com"))
		.await;

	assert_eq!(d.mappings(first, &tenant).await, 1);
	assert_eq!(d.mappings(second, &tenant).await, 0);
	assert_eq!(d.spent_by(&tenant, "dana@example.com").await, Some(first));
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_deployment_that_does_not_load_demo_data_fulfils_nothing(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), false).await;
	let tenant = d.demo_tenant("Acme Research", "dana").await;
	d.provide(&tenant, "dana@example.com", "dana").await;

	let identity = d.sign_in_as("dana-google", Some("dana@example.com")).await;

	assert_eq!(d.mappings(identity, &tenant).await, 0);
	assert_eq!(d.spent_by(&tenant, "dana@example.com").await, None);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_sign_in_without_a_verified_email_fulfils_nothing(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), true).await;
	let tenant = d.demo_tenant("Acme Research", "dana").await;
	d.provide(&tenant, "dana@example.com", "dana").await;

	let identity = d.sign_in_as("dana-google", None).await;

	assert_eq!(d.mappings(identity, &tenant).await, 0);
	assert_eq!(d.spent_by(&tenant, "dana@example.com").await, None);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_identity_that_exists_before_the_load_is_mapped_at_load_time(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), true).await;
	let tenant = d.demo_tenant("Acme Research", "alex").await;
	// `person` holds alex@example.com and signed in before any load.
	let alex = d.person("alex").await;

	d.provide(&tenant, "alex@example.com", "alex").await;

	assert_eq!(d.mappings(alex.identity, &tenant).await, 1);
	assert_eq!(
		d.spent_by(&tenant, "alex@example.com").await,
		Some(alex.identity)
	);
	// Loading again changes nothing.
	d.provide(&tenant, "alex@example.com", "alex").await;
	assert_eq!(d.mappings(alex.identity, &tenant).await, 1);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn an_address_more_than_one_identity_holds_stays_reserved_at_load_time(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), true).await;
	let tenant = d.demo_tenant("Acme Research", "dana").await;
	let first = d.sign_in_as("dana-google", Some("dana@example.com")).await;
	let second = d
		.sign_in_as("dana-password", Some("dana@example.com"))
		.await;

	d.provide(&tenant, "dana@example.com", "dana").await;

	assert_eq!(d.mappings(first, &tenant).await, 0);
	assert_eq!(d.mappings(second, &tenant).await, 0);
	assert_eq!(d.spent_by(&tenant, "dana@example.com").await, None);
	// The next sign-in of either spends it.
	d.sign_in_as("dana-password", Some("dana@example.com"))
		.await;
	assert_eq!(d.mappings(second, &tenant).await, 1);
	assert_eq!(d.spent_by(&tenant, "dana@example.com").await, Some(second));
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn reservations_in_two_demo_tenants_are_both_fulfilled(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), true).await;
	let acme = d.demo_tenant("Acme Research", "dana").await;
	let globex = d.demo_tenant("Globex", "dana-globex").await;
	d.provide(&acme, "dana@example.com", "dana").await;
	d.provide(&globex, "dana@example.com", "dana-globex").await;

	let identity = d.sign_in_as("dana-google", Some("dana@example.com")).await;

	assert_eq!(d.mappings(identity, &acme).await, 1);
	assert_eq!(d.mappings(identity, &globex).await, 1);
	assert_eq!(d.spent_by(&acme, "dana@example.com").await, Some(identity));
	assert_eq!(
		d.spent_by(&globex, "dana@example.com").await,
		Some(identity)
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_reservation_that_cannot_be_spent_never_blocks_the_sign_in(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), true).await;
	let acme = d.demo_tenant("Acme Research", "dana").await;
	let globex = d.demo_tenant("Globex", "dana-globex").await;
	// The first Reservation names a subject its Tenant no longer has.
	d.provide(&acme, "dana@example.com", "ghost").await;
	d.provide(&globex, "dana@example.com", "dana-globex").await;

	let identity = d.sign_in_as("dana-google", Some("dana@example.com")).await;

	assert_eq!(d.mappings(identity, &acme).await, 0);
	assert_eq!(d.spent_by(&acme, "dana@example.com").await, None);
	assert_eq!(d.mappings(identity, &globex).await, 1);
	assert_eq!(
		d.spent_by(&globex, "dana@example.com").await,
		Some(identity)
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn bootstrap_reserves_both_roles_and_reruns_idempotently(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), false).await;
	let pool = &d.federation.store.pool;
	let config = &d.federation.config;
	let first = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		pool,
		config,
		"dana@example.com",
		"default",
	)
	.await
	.unwrap();
	let second = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		pool,
		config,
		"dana@example.com",
		"default",
	)
	.await
	.unwrap();
	assert_eq!(first.tenant, second.tenant);
	assert!(first.created);
	assert!(!second.created);
	let snapshot = aidash_server::authorization::Authorization { pool: pool.clone() }
		.snapshot(&first.tenant)
		.await
		.unwrap();
	assert!(
		aidash_domain::identity::tenancy::has_unconditional_administrator(
			&snapshot.bundle,
			["owner"]
		)
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM tenants WHERE name = '{}' AND created_by_kind = 'host_access'",
			first.tenant
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM nodes WHERE tenant = '{}' AND created_by_kind = 'host_access'",
			first.tenant
		))
		.await,
		1
	);
	assert_eq!(d.spent_by(&first.tenant, "dana@example.com").await, None);
	assert_eq!(d.count("SELECT count(*) FROM bootstrap_deployment_administrator_reservations WHERE email = 'dana@example.com' AND issuer = 'host_access'").await, 1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM demo_reservations WHERE tenant = '{}' AND email = 'dana@example.com' AND subject = 'owner' AND source = 'host_access'", first.tenant)).await, 1);
	let identity = d
		.sign_in_as("dana-bootstrap", Some("DANA@example.com"))
		.await;
	assert_eq!(d.mappings(identity, &first.tenant).await, 1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_deployment_administrator_grants WHERE identity_id = '{identity}' AND enabled")).await, 1);
	assert_eq!(
		d.count("SELECT count(*) FROM bootstrap_deployment_administrator_reservations")
			.await,
		0
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE actor_kind = 'host_access' AND identity_id = '{identity}' AND action = 'deployment_administrator.grant'")).await, 1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE actor_kind = 'host_access' AND identity_id = '{identity}' AND action = 'demo_reservation.fulfil'")).await, 1);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn bootstrap_grants_an_existing_identity_immediately(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), false).await;
	let identity = d
		.sign_in_as("existing-bootstrap", Some("existing@example.com"))
		.await;
	let result = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		&d.federation.store.pool,
		&d.federation.config,
		"existing@example.com",
		"default",
	)
	.await
	.unwrap();
	assert_eq!(d.mappings(identity, &result.tenant).await, 1);
	let again = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		&d.federation.store.pool,
		&d.federation.config,
		"existing@example.com",
		&result.tenant,
	)
	.await
	.unwrap();
	assert_eq!(again.tenant_administrator, "already mapped");
	assert_eq!(again.deployment_administrator, "already granted");
	assert_eq!(d.mappings(identity, &result.tenant).await, 1);
	let db = d.federation.store.pool.fixture_driver();
	sqlx::query(
		"UPDATE dashboard_mappings SET enabled = false WHERE identity_id = $1 AND tenant = $2",
	)
	.bind(identity)
	.bind(&result.tenant)
	.execute(db)
	.await
	.unwrap();
	sqlx::query(
		"UPDATE dashboard_deployment_administrator_grants SET enabled = false WHERE identity_id = $1",
	)
	.bind(identity)
	.execute(db)
	.await
	.unwrap();
	let recovered = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		&d.federation.store.pool,
		&d.federation.config,
		"existing@example.com",
		&result.tenant,
	)
	.await
	.unwrap();
	assert_eq!(recovered.tenant_administrator, "mapped");
	assert_eq!(recovered.deployment_administrator, "granted");
	assert_eq!(d.mappings(identity, &result.tenant).await, 1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_deployment_administrator_grants WHERE identity_id = '{identity}' AND enabled")).await, 1);
	assert_eq!(
		d.count("SELECT count(*) FROM bootstrap_deployment_administrator_reservations")
			.await,
		0
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn bootstrap_reuses_name_and_refuses_ambiguous_labels(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), false).await;
	let a = d.demo_tenant("Duplicate", "member-a").await;
	let b = d.demo_tenant("Duplicate", "member-b").await;
	let pool = &d.federation.store.pool;
	let config = &d.federation.config;
	let error = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		pool,
		config,
		"admin@example.com",
		"Duplicate",
	)
	.await
	.unwrap_err()
	.to_string();
	assert!(error.contains(&a));
	assert!(error.contains(&b));
	let by_name = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		pool,
		config,
		"admin@example.com",
		&a,
	)
	.await
	.unwrap();
	assert_eq!(by_name.tenant, a);
	assert!(!by_name.created);
	d.finish().await;
}

struct BoundIssuer(String);
impl aidash_application::ports::authorization::dashboard::Issuers for BoundIssuer {
	fn admitted(&self) -> Vec<String> {
		vec![GOOGLE.to_owned()]
	}
	fn restriction(&self, issuer: &str) -> Option<String> {
		(issuer == GOOGLE).then(|| self.0.clone())
	}
	fn restricted_issuer(&self, tenant: &str) -> Option<String> {
		(tenant == self.0).then(|| GOOGLE.to_owned())
	}
}

#[rstest::rstest]
#[tokio::test]
async fn demo_reservation_never_maps_a_tenant_bound_issuer(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), true).await;
	let tenant = d.demo_tenant("Bound", "dana").await;
	let identity = d
		.sign_in_as("bound-person", Some("bound@example.com"))
		.await;
	let policy = aidash_application::ports::authorization::dashboard::AccountPolicy {
		issuers: std::sync::Arc::new(BoundIssuer(tenant.clone())),
		authority: None,
		personal_tenants: false,
		demo_data: true,
	};
	DemoReservation::provide(
		&d.federation.store.pool,
		Some(&policy),
		&tenant,
		"bound@example.com",
		"dana",
	)
	.await
	.unwrap();
	assert_eq!(d.mappings(identity, &tenant).await, 0);
	assert_eq!(d.spent_by(&tenant, "bound@example.com").await, None);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn bootstrap_coexists_with_an_existing_demo_reservation(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), true).await;
	let tenant = d.demo_tenant("Demo", "alex").await;
	let identity = d.sign_in_as("alex-demo", Some("alex@example.com")).await;
	d.provide(&tenant, "alex@example.com", "alex").await;
	let result = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		&d.federation.store.pool,
		&d.federation.config,
		"alex@example.com",
		&tenant,
	)
	.await
	.unwrap();
	assert_eq!(result.tenant, tenant);
	assert_eq!(d.mappings(identity, &tenant).await, 2);
	assert_eq!(d.count(&format!("SELECT count(*) FROM demo_reservations WHERE tenant = '{tenant}' AND email = 'alex@example.com' AND spent_by = '{identity}'")).await, 2);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_deployment_administrator_grants WHERE identity_id = '{identity}' AND enabled")).await, 1);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn bootstrap_with_ambiguous_email_waits_for_the_next_sign_in(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), false).await;
	let a = d
		.sign_in_as("shared-first", Some("shared@example.com"))
		.await;
	let b = d
		.sign_in_as("shared-second", Some("shared@example.com"))
		.await;
	let result = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		&d.federation.store.pool,
		&d.federation.config,
		"shared@example.com",
		"default",
	)
	.await
	.unwrap();
	assert_eq!(result.tenant_administrator, "reserved");
	assert_eq!(result.deployment_administrator, "reserved");
	assert_eq!(d.mappings(a, &result.tenant).await, 0);
	assert_eq!(d.mappings(b, &result.tenant).await, 0);
	d.sign_in_as("shared-second", Some("shared@example.com"))
		.await;
	assert_eq!(d.mappings(b, &result.tenant).await, 1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_deployment_administrator_grants WHERE identity_id = '{b}' AND enabled")).await, 1);
	d.sign_in_as("shared-first", Some("shared@example.com"))
		.await;
	let again = aidash_server::apps::identity::services::bootstrap_command::bootstrap(
		&d.federation.store.pool,
		&d.federation.config,
		"shared@example.com",
		&result.tenant,
	)
	.await
	.unwrap();
	assert_eq!(again.tenant_administrator, "already mapped");
	assert_eq!(again.deployment_administrator, "already granted");
	assert_eq!(d.mappings(a, &result.tenant).await, 0);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_deployment_administrator_grants WHERE identity_id = '{a}' AND enabled")).await, 0);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn concurrent_bootstrap_reuses_one_tenant_and_reservations(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::with_demo_data(runtime, rules(false), false).await;
	let pool = &d.federation.store.pool;
	let config = &d.federation.config;
	let (a, b) = tokio::join!(
		aidash_server::apps::identity::services::bootstrap_command::bootstrap(
			pool,
			config,
			"race@example.com",
			"Race"
		),
		aidash_server::apps::identity::services::bootstrap_command::bootstrap(
			pool,
			config,
			"race@example.com",
			"Race"
		),
	);
	let a = a.unwrap();
	let b = b.unwrap();
	assert_eq!(a.tenant, b.tenant);
	assert_eq!(
		d.count("SELECT count(*) FROM tenants WHERE label = 'Race'")
			.await,
		1
	);
	assert_eq!(d.count("SELECT count(*) FROM bootstrap_deployment_administrator_reservations WHERE email = 'race@example.com'").await, 1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM demo_reservations WHERE tenant = '{}' AND email = 'race@example.com' AND source = 'host_access'", a.tenant)).await, 1);
	d.finish().await;
}
