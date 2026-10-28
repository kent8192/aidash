//! Startup composition, without a database or a reachable authentication peer.
use super::common;
use aidash_domain::data_protection::Region;
use aidash_domain::offering::Offering;
use aidash_server::config::{Config, settings::ProjectSettings};
use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair};
use reinhardt::test::fixtures::temp_dir;
use serde_json::json;

const SHARED: &str = "https://securetoken.google.com/settings-fixture/tenants/shared";
const BOUND: &str = "https://securetoken.google.com/settings-fixture/tenants/bound";
const ISSUER: &str = "https://authentication.example.test";

struct Configuration {
	settings: ProjectSettings,
	_directory: tempfile::TempDir,
}

#[rstest::fixture]
fn configuration(temp_dir: tempfile::TempDir) -> Configuration {
	let mut ca_parameters = CertificateParams::default();
	ca_parameters.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
	let ca_key = KeyPair::generate().unwrap();
	let ca = ca_parameters.self_signed(&ca_key).unwrap();
	let issuer = Issuer::new(ca_parameters, ca_key);
	let key = KeyPair::generate().unwrap();
	let mut parameters = CertificateParams::new(vec!["cloud.example.test".into()]).unwrap();
	parameters.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
	let certificate = parameters.signed_by(&key, &issuer).unwrap();
	let ca_file = temp_dir.path().join("ca.pem");
	let certificate_file = temp_dir.path().join("client.pem");
	let key_file = temp_dir.path().join("client-key.pem");
	std::fs::write(&ca_file, ca.pem()).unwrap();
	std::fs::write(&certificate_file, certificate.pem()).unwrap();
	std::fs::write(&key_file, key.serialize_pem()).unwrap();
	// This is the existing DefaultSource fixture; parsing it opens no database.
	let mut settings = common::settings_for("postgresql://fixture:fixture@127.0.0.1:1/unused");
	settings.deployment.offering = Offering::Cloud;
	// Cloud and Dedicated must declare a Deployment Region.
	settings.deployment.region = Some(Region::Us);
	settings.dashboard = serde_json::from_value(json!({
		"oidc": {"issuer":ISSUER,"client_id":"cloud","client_secret":"settings-fixture-client-secret",
			"public_origin":"https://cloud.example.test"},
		"common_auth": {"grpc_endpoint":"https://127.0.0.1:1","server_domain":"authentication.example.test",
			"ca_certificate_file":ca_file,"client_certificate_file":certificate_file,"client_private_key_file":key_file,
			"source_issuers":{SHARED:"",BOUND:"acme"}}
	})).unwrap();
	Configuration {
		settings,
		_directory: temp_dir,
	}
}

#[rstest::rstest]
#[tokio::test]
async fn cloud_composition_loads_mtls_and_retains_original_source_tenant_restrictions(
	configuration: Configuration,
) {
	let runtime = Config::from_settings(&configuration.settings).unwrap();
	assert!(runtime.common_auth.is_some());
	assert!(runtime.sign_in.is_none());
	let policy = runtime.dashboard_policy().unwrap();
	assert!(policy.authority.is_some());
	assert!(policy.require_mapping(SHARED, "other").is_ok());
	assert!(policy.provisions_personal(SHARED));
	assert!(policy.require_mapping(BOUND, "acme").is_ok());
	assert!(policy.require_mapping(BOUND, "other").is_err());
	assert!(!policy.provisions_personal(BOUND));
	assert!(
		policy.require_identity(ISSUER).is_err(),
		"the downstream issuer cannot replace original pool restrictions"
	);
}

#[rstest::rstest]
#[case::dedicated(
	Offering::Dedicated,
	true,
	"dashboard.common_auth is restricted to Cloud"
)]
#[case::self_hosted(
	Offering::SelfHosted,
	true,
	"dashboard.common_auth is restricted to Cloud"
)]
#[case::missing_oidc(
	Offering::Cloud,
	false,
	"dashboard.common_auth requires dashboard.oidc"
)]
#[tokio::test]
async fn common_composition_rejects_another_offering_or_missing_oidc(
	mut configuration: Configuration,
	#[case] offering: Offering,
	#[case] oidc: bool,
	#[case] refusal: &str,
) {
	configuration.settings.deployment.offering = offering;
	if !oidc {
		configuration.settings.dashboard.oidc = None;
	}
	let error = Config::from_settings(&configuration.settings)
		.err()
		.expect("configuration must fail before connecting");
	assert_eq!(error.to_string(), refusal);
}

enum InvalidTls {
	MissingKey,
	InvalidCa,
	InvalidKey,
	PlaintextEndpoint,
}

#[rstest::rstest]
#[case::missing_key(InvalidTls::MissingKey)]
#[case::invalid_ca(InvalidTls::InvalidCa)]
#[case::invalid_key(InvalidTls::InvalidKey)]
#[case::plaintext(InvalidTls::PlaintextEndpoint)]
#[tokio::test]
async fn common_composition_refuses_missing_or_invalid_transport_material(
	mut configuration: Configuration,
	#[case] invalid: InvalidTls,
) {
	let tls = configuration
		.settings
		.dashboard
		.common_auth
		.as_mut()
		.unwrap();
	let refusal = match invalid {
		InvalidTls::MissingKey => {
			std::fs::remove_file(&tls.client_private_key_file).unwrap();
			"common authentication TLS material cannot be read"
		}
		InvalidTls::InvalidCa => {
			std::fs::write(
				&tls.ca_certificate_file,
				"-----BEGIN CERTIFICATE-----\nnot_base64!\n-----END CERTIFICATE-----\n",
			)
			.unwrap();
			"invalid common authentication mTLS endpoint"
		}
		InvalidTls::InvalidKey => {
			std::fs::write(
				&tls.client_private_key_file,
				"private-key-fixture-do-not-reflect",
			)
			.unwrap();
			"invalid common authentication mTLS endpoint"
		}
		InvalidTls::PlaintextEndpoint => {
			tls.grpc_endpoint = "http://127.0.0.1:1".into();
			"invalid common authentication mTLS endpoint"
		}
	};
	let error = Config::from_settings(&configuration.settings)
		.err()
		.expect("invalid TLS must never fall back to another sign-in");
	assert_eq!(error.to_string(), refusal);
	assert!(
		!error
			.to_string()
			.contains("private-key-fixture-do-not-reflect")
	);
}

#[rstest::rstest]
#[case::empty_issuers(true)]
#[case::invalid_tenant(false)]
#[tokio::test]
async fn common_composition_requires_explicit_sources_and_valid_bound_tenants(
	mut configuration: Configuration,
	#[case] empty: bool,
) {
	let common = configuration
		.settings
		.dashboard
		.common_auth
		.as_mut()
		.unwrap();
	if empty {
		common.source_issuers.clear();
	} else {
		common
			.source_issuers
			.insert(BOUND.into(), "invalid tenant".into());
	}
	let error = Config::from_settings(&configuration.settings)
		.err()
		.expect("source policy must be valid before any request");
	assert!(matches!(error, aidash_server::Error::Invalid(_)));
}

#[rstest::rstest]
#[case::cloud(Offering::Cloud)]
#[case::dedicated(Offering::Dedicated)]
#[case::self_hosted(Offering::SelfHosted)]
fn no_common_configuration_preserves_independent_deployment_startup(#[case] offering: Offering) {
	let mut settings = common::settings_for("postgresql://fixture:fixture@127.0.0.1:1/unused");
	settings.deployment.offering = offering;
	if offering != Offering::SelfHosted {
		settings.deployment.region = Some(Region::Us);
	}
	let runtime = Config::from_settings(&settings).unwrap();
	assert!(runtime.common_auth.is_none());
	assert!(runtime.common_issuers.is_none());
	assert!(runtime.oidc.is_none());
	assert!(runtime.dashboard_policy().is_none());
}
