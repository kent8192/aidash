use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn gcip_config() -> GcipConfig {
	serde_json::from_value(serde_json::json!({"project_id":"fixture-project","web_api_key":"public-key","public_origin":"https://dashboard.example.test/","tenant_bindings":{"pool-a":"acme"}})).unwrap()
}
#[rstest]
fn gcip_bindings_are_one_to_one_and_can_be_removed(mut gcip_config: GcipConfig) {
	assert_eq!(
		gcip_config.normalized().unwrap().public_origin,
		"https://dashboard.example.test"
	);
	gcip_config
		.tenant_bindings
		.insert("pool-b".into(), "acme".into());
	assert!(gcip_config.normalized().is_err());
	gcip_config.tenant_bindings.clear();
	assert!(gcip_config.normalized().is_ok());
}
#[rstest]
#[case("http://dashboard.example.test")]
#[case("http://127.0.0.1:8080")]
#[case("http://localhost:8080")]
#[case("https://dashboard.example.test/path")]
#[case("https://user:password@dashboard.example.test")]
fn gcip_rejects_invalid_origins(mut gcip_config: GcipConfig, #[case] origin: &str) {
	gcip_config.public_origin = origin.into();
	assert!(gcip_config.normalized().is_err());
}
#[rstest]
#[case("acme.com", "pool-b", "GCIP Sign-in Domain requires a Tenant Binding")]
#[case(
	"ACME.com",
	"pool-a",
	"GCIP Sign-in Domains must be lowercase ASCII DNS names"
)]
#[case(
	"bücher.example",
	"pool-a",
	"GCIP Sign-in Domains must be lowercase ASCII DNS names"
)]
#[case(
	"localhost",
	"pool-a",
	"GCIP Sign-in Domains must be lowercase ASCII DNS names"
)]
#[case(
	"192.0.2.1",
	"pool-a",
	"GCIP Sign-in Domains must be lowercase ASCII DNS names"
)]
#[case(
	"acme.0x1f",
	"pool-a",
	"GCIP Sign-in Domains must be lowercase ASCII DNS names"
)]
fn gcip_rejects_noncanonical_or_unbound_sign_in_domains(
	mut gcip_config: GcipConfig,
	#[case] domain: &str,
	#[case] pool: &str,
	#[case] reason: &str,
) {
	gcip_config
		.sign_in_domains
		.insert(domain.into(), pool.into());
	assert!(matches!(gcip_config.normalized(), Err(Error::Invalid(message)) if message == reason));
	gcip_config.sign_in_domains = [("xn--bcher-kva.example".into(), "pool-a".into())].into();
	assert!(gcip_config.normalized().is_ok());
}
#[rstest]
#[case::default_keeps_google_helper(None, "fixture-project.firebaseapp.com")]
#[case::firebase(Some("firebase"), "fixture-project.firebaseapp.com")]
#[case::public_origin(Some("public_origin"), "dashboard.example.test")]
fn auth_domain_follows_the_auth_helper(#[case] helper: Option<&str>, #[case] expected: &str) {
	let mut value = serde_json::json!({"project_id":"fixture-project","web_api_key":"public-key","public_origin":"https://dashboard.example.test/","tenant_bindings":{}});
	if let Some(helper) = helper {
		value["auth_helper"] = helper.into();
	}
	let config: GcipConfig = serde_json::from_value(value).unwrap();
	assert_eq!(config.auth_domain(), expected);
}
#[rstest]
fn public_origin_auth_domain_keeps_a_non_default_port(mut gcip_config: GcipConfig) {
	gcip_config.public_origin = "https://dashboard.example.test:8443".into();
	gcip_config.auth_helper = AuthHelper::PublicOrigin;
	assert_eq!(gcip_config.auth_domain(), "dashboard.example.test:8443");
}
#[rstest]
fn unknown_auth_helper_is_rejected() {
	let value = serde_json::json!({"project_id":"fixture-project","web_api_key":"public-key","public_origin":"https://dashboard.example.test","tenant_bindings":{},"auth_helper":"auth.example.test"});
	assert!(serde_json::from_value::<GcipConfig>(value).is_err());
}

#[rstest::rstest]
#[case::other_section(json!({"dashboard": {"gcip": {}}, "node": {"api_token": "override"}}))]
#[case::other_issuer(json!({"dashboard": {"gcip": {}, "oidc": {}}}))]
#[case::missing_file(Value::Null)]
fn invalid_managed_source_is_not_silently_ignored(#[case] mut value: Value) {
	let directory = tempfile::tempdir().unwrap();
	let path = directory.path().join("settings.json");
	if !value.is_null() {
		value["dashboard"]["gcip"] = json!({
			"project_id": "fixture-project",
			"web_api_key": "public",
			"public_origin": "https://test.aidash.run",
			"tenant_bindings": {"Pool-X": "acme"}
		});
		std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
	}
	let source = ManagedGcipSource { path: Some(path) };
	assert!(source.load_scoped().is_err());
}

#[rstest]
#[case("acme.com", Some("acme.com"))]
#[case("ACME.Com", Some("acme.com"))]
#[case("bücher.example", Some("xn--bcher-kva.example"))]
#[case("xn--bcher-kva.example", Some("xn--bcher-kva.example"))]
#[case("localhost", None)]
#[case("acme.com.", None)]
#[case("-acme.com", None)]
#[case("ac_me.com", None)]
#[case("192.0.2.1", None)]
#[case("[::1]", None)]
#[case("%61cme.com", None)]
#[case("acme%2ecom", None)]
#[case("acme.com/", None)]
#[case("acme\\.com", None)]
#[case("", None)]
fn sign_in_domains_are_canonical_dns_names(#[case] raw: &str, #[case] expected: Option<&str>) {
	assert_eq!(sign_in_domain(raw).as_deref(), expected);
}

#[rstest]
#[case("alice@acme.com", Some("pool-a"))]
#[case("  Alice@ACME.com ", Some("pool-a"))]
#[case("bob@bücher.example", Some("pool-b"))]
#[case("alice@eng.acme.com", None)]
#[case("alice@other.com", None)]
#[case("acme.com", None)]
#[case("@acme.com", None)]
#[case("a@b@acme.com", None)]
#[case("alice@", None)]
#[case("o'neil+tag@acme.com", Some("pool-a"))]
#[case("first.last@acme.com", Some("pool-a"))]
#[case("bad local@acme.com", None)]
#[case(".alice@acme.com", None)]
#[case("alice.@acme.com", None)]
#[case("alice..bob@acme.com", None)]
#[case("\"alice\"@acme.com", None)]
#[case("alice(x)@acme.com", None)]
#[case("álice@acme.com", None)]
#[case("alice@%61cme.com", None)]
#[case("alice@acme%2ecom", None)]
#[case("alice@b%C3%BCcher.example", None)]
fn email_routes_only_by_exact_sign_in_domain(#[case] email: &str, #[case] expected: Option<&str>) {
	let domains = std::collections::BTreeMap::from([
		("acme.com".to_owned(), "pool-a".to_owned()),
		("xn--bcher-kva.example".to_owned(), "pool-b".to_owned()),
	]);
	assert_eq!(routed_gcip_tenant(&domains, email), expected);
}

#[rstest]
fn overlong_addresses_never_route() {
	let domains = std::collections::BTreeMap::from([("acme.com".to_owned(), "pool-a".to_owned())]);
	assert_eq!(
		routed_gcip_tenant(&domains, &format!("{}@acme.com", "a".repeat(65))),
		None
	);
}

#[rstest]
fn a_shared_pool_needs_no_binding_and_has_no_restriction(mut gcip_config: GcipConfig) {
	gcip_config.tenant_bindings.clear();
	gcip_config.shared_pool = Some("pool-s".into());
	gcip_config.providers.insert(
		"pool-s".into(),
		vec!["google.com".into(), "password".into()],
	);
	gcip_config.mfa_mandatory.insert("pool-s".into());
	let normalized = gcip_config.normalized().unwrap();
	assert_eq!(normalized.status("pool-s"), Some(PoolStatus::Shared));
	assert_eq!(normalized.status("pool-a"), None);
}

#[rstest]
fn the_shared_pool_and_the_bound_pools_are_disjoint(mut gcip_config: GcipConfig) {
	gcip_config.shared_pool = Some("pool-a".into());
	assert_eq!(
		gcip_config.normalized().unwrap_err().to_string(),
		"the GCIP Shared Pool cannot also have a Tenant Binding"
	);
}

#[rstest]
#[case::providers("providers")]
#[case::password("password_sign_up")]
#[case::mfa("mfa_mandatory")]
#[case::domain("sign_in_domains")]
fn pool_settings_name_only_shared_or_bound_pools(
	mut gcip_config: GcipConfig,
	#[case] setting: &str,
) {
	gcip_config.shared_pool = Some("pool-s".into());
	let pool = "pool-unknown".to_owned();
	match setting {
		"providers" => {
			gcip_config.providers.insert(pool, vec!["password".into()]);
		}
		"password_sign_up" => {
			gcip_config.password_sign_up.insert(pool);
		}
		"mfa_mandatory" => {
			gcip_config.mfa_mandatory.insert(pool);
		}
		_ => {
			// A Sign-in Domain routes only to a bound pool, never the Shared Pool.
			gcip_config
				.sign_in_domains
				.insert("acme.example".into(), "pool-s".into());
		}
	}
	assert!(gcip_config.normalized().is_err(), "{setting}");
}

#[rstest]
fn gcip_session_lifetime_defaults_and_bound(mut gcip_config: GcipConfig) {
	assert_eq!(gcip_config.session_idle_seconds, 2_592_000);
	assert_eq!(gcip_config.session_absolute_seconds, 34_560_000);
	gcip_config.session_idle_seconds = 34_560_000;
	assert!(gcip_config.normalized().is_ok());
	gcip_config.session_absolute_seconds = 34_560_001;
	assert!(gcip_config.normalized().is_err());
	gcip_config.session_idle_seconds = 34_560_001;
	gcip_config.session_absolute_seconds = 34_560_000;
	assert!(gcip_config.normalized().is_err());
}
