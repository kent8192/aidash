//! Recheck browser authority after authentication and before a native mutation.
use super::{Workbench, captured_actor_application, common, request, workbench};
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::apps::identity::models::{
	DashboardIdentity, DashboardMapping, DashboardSession,
};
use aidash_server::authorization::Authorization;
use aidash_server::config::OidcConfig;
use chrono::{Duration, Utc};
use http::{HeaderMap, Method};
use reinhardt::db::orm::Model;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[rstest::fixture]
async fn browser_workbench(
	#[default("https://accounts.google.com")] issuer: &str,
	#[future(awt)] workbench: Workbench,
) -> (Workbench, HeaderMap, DashboardSession) {
	let mut wb = workbench;
	wb.f.config.oidc = Some(OidcConfig {
		issuer: issuer.into(),
		client_id: "aidash".into(),
		client_secret: "local-test-secret".into(),
		public_origin: "https://localhost:8080".into(),
		session_absolute_seconds: 43_200,
		session_idle_seconds: 1_800,
	});
	let lease = reinhardt::db::orm::DatabaseConnectionLease::register(wb.f.store.pool.connection())
		.unwrap();
	let mut db = lease.handle();
	let now = Utc::now();
	let identity = DashboardIdentity::build()
		.id(Uuid::new_v4())
		.issuer(issuer)
		.subject("browser-fixture")
		.valid_since(None)
		.verified_email(None)
		.display_name(None)
		.last_valid_at(Some(now))
		.disabled_at(None)
		.display_observed_at(None)
		.display_erased_at(None)
		.display_erased_by(None)
		.finish();
	DashboardIdentity::objects()
		.create_with_conn(&mut db, &identity)
		.await
		.unwrap();
	let session = DashboardSession::build()
		.id(Uuid::new_v4())
		.token_hash(common::browser_session_digest(b"browser-authority-session"))
		.csrf_hash(Sha256::digest(b"browser-authority-csrf").to_vec())
		.identity(identity.id)
		.provider_sid(None)
		.created_at(now)
		.auth_time(None)
		.last_activity_at(now)
		.expires_at(now + Duration::hours(12))
		.revoked_at(None)
		.desktop(false)
		.desktop_idle_seconds(None)
		.access_expires_at(None)
		.finish();
	let session = DashboardSession::objects()
		.create_with_conn(&mut db, &session)
		.await
		.unwrap();
	let credential = Authorization {
		pool: wb.f.store.pool.clone(),
	}
	.credentials("acme")
	.await
	.unwrap()
	.remove(0);
	let mapping = DashboardMapping::build()
		.id(Uuid::new_v4())
		.identity(identity.id)
		.tenant("acme")
		.subject("alice")
		.credential(credential.id)
		.enabled(true)
		.revision(1)
		.finish();
	// Mappings hold a row-level policy; seed this one in the fixture scope.
	wb.f.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			DashboardMapping::objects()
				.create_with_conn(tx, &mapping)
				.await?;
			Ok(())
		})
		.await
		.unwrap();
	let mut headers = HeaderMap::new();
	headers.insert(
		"cookie",
		"__Host-aidash-session=browser-authority-session"
			.parse()
			.unwrap(),
	);
	headers.insert("origin", "https://localhost:8080".parse().unwrap());
	headers.insert("x-aidash-csrf", "browser-authority-csrf".parse().unwrap());
	headers.insert(
		"x-aidash-context",
		format!("mapping:{}:{}", mapping.tenant, mapping.id)
			.parse()
			.unwrap(),
	);
	(wb, headers, session)
}

#[rstest::rstest]
#[case::google_status_age("https://accounts.google.com", "status_age", 200)]
#[case::generic_status_age("http://localhost/issuer", "status_age", 200)]
#[case::logout("https://accounts.google.com", "logout", 401)]
#[case::absolute_expiry("https://accounts.google.com", "expired", 401)]
#[case::idle_expiry("https://accounts.google.com", "idle", 401)]
#[case::disabled_identity("https://accounts.google.com", "disabled", 403)]
#[tokio::test]
async fn native_draft_mutations_recheck_browser_authority(
	#[case] _issuer: &str,
	#[case] change: &str,
	#[case] expected_status: u16,
	#[future(awt)]
	#[with(_issuer)]
	browser_workbench: (Workbench, HeaderMap, DashboardSession),
) {
	let (wb, headers, mut session) = browser_workbench;
	let (actor, _) =
		aidash_server::dashboard_auth::actor_from_headers(&wb.f, &headers, &Method::GET)
			.await
			.unwrap();
	// Act: capture the actor after applying this scenario authority configuration.
	let app = captured_actor_application(&wb.f, actor).await;
	let lease = reinhardt::db::orm::DatabaseConnectionLease::register(wb.f.store.pool.connection())
		.unwrap();
	let mut db = lease.handle();
	let now = Utc::now();
	match change {
		"logout" => {
			// Act: rebuild the browser context after the scenario authority configuration.
			let browser = common::application(wb.f.clone()).await;
			let client = browser.client();
			for (name, value) in &headers {
				client
					.set_header(name.as_str(), value.to_str().unwrap())
					.await
					.unwrap();
			}
			assert_eq!(
				client
					.post("/auth/logout", &Value::Null, "json")
					.await
					.unwrap()
					.status_code(),
				204
			);
		}
		"expired" | "idle" => {
			if change == "expired" {
				session.expires_at = now - Duration::seconds(1);
			} else {
				session.last_activity_at = now - Duration::hours(1);
			}
			DashboardSession::objects()
				.update_with_conn(&mut db, &session)
				.await
				.unwrap();
		}
		"status_age" | "disabled" => {
			let mut identity = DashboardIdentity::objects()
				.get(session.identity_id())
				.get_with_db(&mut db)
				.await
				.unwrap();
			if change == "disabled" {
				identity.disabled_at = Some(now);
			} else {
				identity.last_valid_at = Some(now - Duration::hours(1));
			}
			DashboardIdentity::objects()
				.update_with_conn(&mut db, &identity)
				.await
				.unwrap();
		}
		_ => panic!("unknown authority change: {change}"),
	}
	let (status, result) = request(&app, &wb.token, "PUT", &wb.path(), json!({"expected_revision":1,"entry":wb.draft["entry"],"documents":[],"release_notes":"Browser authority rechecked"})).await;
	assert_eq!(status, expected_status, "{change}: {result}");
	let (revision, release_notes): (i64, String) =
		sqlx::query_as("SELECT revision, release_notes FROM agent_drafts WHERE id = $1")
			.bind(uuid::Uuid::parse_str(wb.draft["id"].as_str().unwrap()).unwrap())
			.fetch_one(wb.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(revision, if expected_status == 200 { 2 } else { 1 });
	assert_eq!(
		release_notes,
		if expected_status == 200 {
			"Browser authority rechecked"
		} else {
			""
		}
	);
	wb.cleanup().await;
}

struct RemovedBinding;
impl aidash_application::ports::authorization::dashboard::Issuers for RemovedBinding {
	fn admitted(&self) -> Vec<String> {
		vec![]
	}
	fn restriction(&self, _: &str) -> Option<String> {
		None
	}
	fn restricted_issuer(&self, _: &str) -> Option<String> {
		None
	}
	fn retires_refused(&self) -> bool {
		true
	}
}

#[rstest::rstest]
#[case::limits("/api/workbench/test-limits/acme", "GET")]
#[case::profiles("/api/workbench/test-profiles/acme", "GET")]
#[case::profile_write("/api/workbench/test-profiles/acme/new", "PUT")]
#[tokio::test]
async fn a_removed_binding_disables_the_identity_before_workbench_authority_locks(
	#[case] path: &str,
	#[case] method: &str,
	#[future(awt)] browser_workbench: (Workbench, HeaderMap, DashboardSession),
) {
	use aidash_application::ports::authorization::dashboard::AccountPolicy;
	use aidash_server::apps::registry::workbench::views::{profile, test};
	let (mut wb, _, session) = browser_workbench;
	// A fresh self-issued bearer acts through the existing browser Mapping.
	let authorization = Authorization {
		pool: wb.f.store.pool.clone(),
	};
	let issued = authorization
		.issue_credential("acme", "alice", 3600, "fixture")
		.await
		.unwrap();
	wb.f.store
		.pool
		.atomic(&TEST_FIXTURE, async |tx| {
			let mapping = DashboardMapping::objects()
				.filter(DashboardMapping::field_identity_id().eq(session.identity_id()))
				.get_with_db(tx)
				.await?;
			let mut credential =
				aidash_server::apps::identity::models::AuthorizationCredential::objects()
					.get(issued.credential.id)
					.get_with_db(tx)
					.await?;
			credential.mapping_id = Some(mapping.id);
			aidash_server::apps::identity::models::AuthorizationCredential::objects()
				.update_with_conn(tx, &credential)
				.await?;
			Ok(())
		})
		.await
		.unwrap();
	let actor = authorization.authenticate(&issued.token).await.unwrap();
	wb.f.store =
		wb.f.store
			.clone()
			.with_dashboard_policy(Some(AccountPolicy {
				issuers: std::sync::Arc::new(RemovedBinding),
				authority: None,
				personal_tenants: false,
				demo_data: false,
			}));
	let app = common::application_with(wb.f.clone(), |_| {
		reinhardt::ServerRouter::new()
			.endpoint(test::tenant_limits)
			.endpoint(profile::administered)
			.endpoint(profile::put)
			.with_middleware(super::CapturedActor(actor.clone()))
	})
	.await;
	let (status, response) = request(
		&app,
		&wb.token,
		method,
		path,
		json!({"expected_revision":0,"enabled":true,"rules":[]}),
	)
	.await;
	assert_eq!(status, 403, "{response}");
	let disabled =
		wb.f.store
			.pool
			.atomic(&TEST_FIXTURE, async |tx| {
				Ok(DashboardIdentity::objects()
					.get(session.identity_id())
					.get_with_db(tx)
					.await?
					.disabled_at)
			})
			.await
			.unwrap();
	assert!(
		disabled.is_some(),
		"refusal must durably disable the removed identity"
	);
	wb.cleanup().await;
}
