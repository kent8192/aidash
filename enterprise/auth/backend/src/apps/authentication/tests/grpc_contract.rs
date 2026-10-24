//! Exercise the actual Auth RPC policies and PostgreSQL state through mTLS.
use aidash_application::{Error as ApplicationError, ports::accounts::Authority};
use aidash_integrations::authentication::{
	Client, Settings,
	proto::{
		AccountAdministrationRequest, RevokeProductSessionRequest,
		account_administration_client::AccountAdministrationClient,
		session_revocation_client::SessionRevocationClient,
	},
};
use chrono::Utc;
use rcgen::{
	BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
	KeyUsagePurpose, SanType,
};
use reinhardt::grpc::{GrpcClientTlsConfig, tonic};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

struct Running(tokio::task::JoinHandle<crate::Result<()>>);
impl Drop for Running {
	fn drop(&mut self) {
		self.0.abort();
	}
}

#[tokio::test]
async fn actual_auth_grpc_preserves_session_stop_and_administrator_boundaries() {
	let fixture = super::fixture(|_| {}).await;
	let mut params = CertificateParams::default();
	params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
	params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
	let key = KeyPair::generate().unwrap();
	let ca = params.self_signed(&key).unwrap().pem();
	let issuer = Issuer::new(params, key);
	let leaf = |uri: Option<&str>, server: bool| {
		let mut params = CertificateParams::new(vec!["authentication.internal".into()]).unwrap();
		params.extended_key_usages = vec![if server {
			ExtendedKeyUsagePurpose::ServerAuth
		} else {
			ExtendedKeyUsagePurpose::ClientAuth
		}];
		if let Some(uri) = uri {
			params
				.subject_alt_names
				.push(SanType::URI(uri.try_into().unwrap()));
		}
		let key = KeyPair::generate().unwrap();
		(
			params.signed_by(&key, &issuer).unwrap().pem(),
			key.serialize_pem(),
		)
	};
	let files = reinhardt::test::fixtures::temp_dir();
	let write = |name: &str, value: &str| {
		let path = files.path().join(name);
		std::fs::write(&path, value).unwrap();
		path
	};
	let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let address = reservation.local_addr().unwrap();
	let endpoint = format!("https://{address}");
	let (server_cert, server_key) = leaf(None, true);
	let mut auth = (*fixture.auth).clone();
	let mut config = (*auth.config).clone();
	config.grpc.listen = address;
	config.grpc.certificate_file = write("server.pem", &server_cert);
	config.grpc.private_key_file = write("server-key.pem", &server_key);
	config.grpc.client_ca_file = write("ca.pem", &ca);
	config.grpc.product_peers = [("cloud".into(), "spiffe://aidash/cloud".into())].into();
	config.grpc.administrator_peers = vec!["spiffe://aidash/auth-admin".into()];
	auth.config = Arc::new(config);
	let running = Running(tokio::spawn(crate::grpc::serve_on(
		Arc::new(auth),
		reservation,
	)));
	tokio::time::timeout(Duration::from_secs(5), async {
		loop {
			if tokio::net::TcpStream::connect(address).await.is_ok() {
				break;
			}
			assert!(!running.0.is_finished(), "actual Auth RPC server exited");
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	})
	.await
	.unwrap();
	let (cloud_cert, cloud_key) = leaf(Some("spiffe://aidash/cloud"), false);
	let client = Client::new(Settings {
		endpoint: endpoint.clone(),
		server_domain: "authentication.internal".into(),
		ca_certificate: ca.as_bytes().to_vec(),
		client_certificate: cloud_cert.as_bytes().to_vec(),
		client_private_key: cloud_key.as_bytes().to_vec(),
		product_client_id: "cloud".into(),
	})
	.unwrap();
	let browser = fixture
		.auth
		.issue_session(fixture.identity.id, "cloud", Utc::now())
		.await
		.unwrap();
	// The real interactive response deliberately has no durable suspension epoch.
	// A mock returning Some(0) previously hid a client/server contract mismatch.
	let admitted = client
		.interactive(fixture.identity.id, browser.id)
		.await
		.unwrap();
	assert_eq!(admitted.suspension_epoch, None);
	assert_eq!(admitted.account_id, fixture.account.id.to_string());
	assert!(matches!(
		client
			.validate_interactive(Uuid::new_v4(), browser.id)
			.await,
		Err(ApplicationError::Forbidden)
	));
	let original = client.origin_binding(fixture.identity.id).await.unwrap();
	assert_eq!(original.suspension_epoch, Some(0));
	let cloud_channel =
		GrpcClientTlsConfig::new(&ca, &cloud_cert, &cloud_key, "authentication.internal")
			.connect(&endpoint)
			.await
			.unwrap();
	let command = AccountAdministrationRequest {
		account_id: fixture.account.id.to_string(),
		audit_reason: "Real RPC contract regression".into(),
	};
	let forbidden = AccountAdministrationClient::new(cloud_channel.clone())
		.suspend_account(command.clone())
		.await
		.unwrap_err();
	assert_eq!(forbidden.code(), tonic::Code::PermissionDenied);
	let desktop = client
		.desktop_session(
			browser.id,
			fixture.identity.id,
			Uuid::new_v4(),
			86_400,
			1800,
		)
		.await
		.unwrap();
	let revoked = SessionRevocationClient::new(cloud_channel)
		.revoke_product_session(RevokeProductSessionRequest {
			product_session_id: browser.id.to_string(),
			product_client_id: "cloud".into(),
		})
		.await
		.unwrap()
		.into_inner();
	assert!(revoked.completed);
	assert!(matches!(
		client
			.validate_interactive(fixture.identity.id, browser.id)
			.await,
		Err(ApplicationError::Forbidden)
	));
	client
		.validate_interactive(fixture.identity.id, desktop)
		.await
		.unwrap();
	client.validate_bound_origin(&original).await.unwrap();
	let (admin_cert, admin_key) = leaf(Some("spiffe://aidash/auth-admin"), false);
	let admin_channel =
		GrpcClientTlsConfig::new(&ca, admin_cert, admin_key, "authentication.internal")
			.connect(&endpoint)
			.await
			.unwrap();
	let mut administrator = AccountAdministrationClient::new(admin_channel);
	assert!(
		administrator
			.suspend_account(command.clone())
			.await
			.unwrap()
			.into_inner()
			.completed
	);
	assert!(matches!(
		client
			.validate_interactive(fixture.identity.id, desktop)
			.await,
		Err(ApplicationError::Forbidden)
	));
	assert!(matches!(
		client.validate_bound_origin(&original).await,
		Err(ApplicationError::Forbidden)
	));
	assert!(
		administrator
			.restore_account(command)
			.await
			.unwrap()
			.into_inner()
			.completed
	);
	assert!(matches!(
		client
			.validate_interactive(fixture.identity.id, desktop)
			.await,
		Err(ApplicationError::Forbidden)
	));
	assert!(matches!(
		client.validate_bound_origin(&original).await,
		Err(ApplicationError::Forbidden)
	));
	let fresh = client.origin_binding(fixture.identity.id).await.unwrap();
	assert_eq!(fresh.suspension_epoch, Some(1));
}
