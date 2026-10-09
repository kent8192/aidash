//! Recheck browser authority after authentication and before a native mutation.
use super::{Workbench, captured_actor_application, common, request, workbench};
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
) -> (Workbench, HeaderMap, DashboardSession) {
	let mut wb = workbench().await;
	wb.f.config.oidc = Some(OidcConfig {
		issuer: issuer.into(),
		client_id: "aidash".into(),
		client_secret: "local-test-secret".into(),
		public_origin: "http://localhost:8080".into(),
		keycloak_admin_url: String::new(),
		status_client_id: String::new(),
		status_client_secret: String::new(),
		session_absolute_seconds: 43_200,
		session_idle_seconds: 1_800,
	});
	let mut db = wb.f.registry.db;
	let now = Utc::now();
	let identity = DashboardIdentity::build()
		.id(Uuid::new_v4())
		.issuer(issuer)
		.subject("browser-fixture")
		.gcip_tenant("")
		.valid_since(None)
		.verified_email(None)
		.display_name(None)
		.last_valid_at(Some(now))
		.disabled_at(None)
		.finish();
	DashboardIdentity::objects()
		.create_with_conn(&mut db, &identity)
		.await
		.unwrap();
	let session = DashboardSession::build()
		.id(Uuid::new_v4())
		.token_hash(Sha256::digest(b"browser-authority-session").to_vec())
		.csrf_hash(Sha256::digest(b"browser-authority-csrf").to_vec())
		.identity_id(identity.id)
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
		.identity_id(identity.id)
		.tenant("acme")
		.subject("alice")
		.credential_id(credential.id)
		.enabled(true)
		.revision(1)
		.finish();
	DashboardMapping::objects()
		.create_with_conn(&mut db, &mapping)
		.await
		.unwrap();
	let mut headers = HeaderMap::new();
	headers.insert(
		"cookie",
		"aidash-session=browser-authority-session".parse().unwrap(),
	);
	headers.insert("origin", "http://localhost:8080".parse().unwrap());
	headers.insert("x-aidash-csrf", "browser-authority-csrf".parse().unwrap());
	headers.insert(
		"x-aidash-context",
		format!("mapping:{}", mapping.id).parse().unwrap(),
	);
	(wb, headers, session)
}

#[rstest::rstest]
#[case::google_status_age("https://accounts.google.com", "status_age", 200)]
#[case::keycloak_status_age("http://localhost/realms/fixture", "status_age", 503)]
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
	let app = captured_actor_application(&wb.f, actor).await;
	let mut db = wb.f.registry.db;
	let now = Utc::now();
	match change {
		"logout" => {
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
	let (status, saved) = request(
		&wb.app,
		&wb.f.config.api_token,
		"GET",
		&wb.path(),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(
		saved["revision"],
		if expected_status == 200 { 2 } else { 1 }
	);
	assert_eq!(
		saved["release_notes"],
		if expected_status == 200 {
			"Browser authority rechecked"
		} else {
			""
		}
	);
	wb.cleanup().await;
}
