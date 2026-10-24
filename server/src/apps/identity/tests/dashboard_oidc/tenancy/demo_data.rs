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
