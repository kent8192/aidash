use super::*;
use aidash_integrations::authentication::{
	Client, Settings,
	proto::{
		self,
		authentication_validity_server::{AuthenticationValidity, AuthenticationValidityServer},
		session_revocation_server::{SessionRevocation, SessionRevocationServer},
	},
};
use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair};
use reinhardt::grpc::{
	GrpcServerTlsConfig,
	tonic::{self, Request, Response, Status},
};
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};
use tokio_stream::wrappers::TcpListenerStream;

#[derive(Clone)]
struct AuthenticationPeer {
	source: Uuid,
	account: Uuid,
	browser: Uuid,
	desktop: Uuid,
	browser_expired: Arc<AtomicBool>,
	global_logout: Arc<AtomicBool>,
}
impl AuthenticationPeer {
	fn validity(&self, active: bool) -> proto::ValidityResponse {
		proto::ValidityResponse {
			active,
			reason: if active { "" } else { "session_invalid" }.into(),
			source_identity_id: self.source.to_string(),
			account_id: self.account.to_string(),
			source_issuer: "https://accounts.google.com".into(),
			source_subject: "desktop-fixture".into(),
			suspension_epoch: Some(0),
			source_disable_epoch: 0,
		}
	}
}
#[tonic::async_trait]
impl AuthenticationValidity for AuthenticationPeer {
	async fn validate_interactive(
		&self,
		request: Request<proto::ValidateInteractiveRequest>,
	) -> Result<Response<proto::ValidityResponse>, Status> {
		let request = request.into_inner();
		let session = Uuid::parse_str(&request.product_session_id).unwrap();
		assert_eq!(request.external_identity_id, self.source.to_string());
		assert_eq!(request.product_client_id, "fixture");
		let active = !self.global_logout.load(Ordering::SeqCst)
			&& (session == self.desktop
				|| (session == self.browser && !self.browser_expired.load(Ordering::SeqCst)));
		Ok(Response::new(self.validity(active)))
	}
	async fn validate_origin(
		&self,
		_: Request<proto::ValidateOriginRequest>,
	) -> Result<Response<proto::ValidityResponse>, Status> {
		Ok(Response::new(self.validity(true)))
	}
}
#[tonic::async_trait]
impl SessionRevocation for AuthenticationPeer {
	async fn revoke_product_session(
		&self,
		_: Request<proto::RevokeProductSessionRequest>,
	) -> Result<Response<proto::RevocationResponse>, Status> {
		panic!("ordinary browser logout must stay local and keep the independent Desktop session");
	}
	async fn revoke_all_sessions(
		&self,
		request: Request<proto::RevokeAllSessionsRequest>,
	) -> Result<Response<proto::RevocationResponse>, Status> {
		assert_eq!(
			request.into_inner().product_session_id,
			self.desktop.to_string()
		);
		self.global_logout.store(true, Ordering::SeqCst);
		Ok(Response::new(proto::RevocationResponse { completed: true }))
	}
	async fn create_desktop_session(
		&self,
		request: Request<proto::CreateDesktopSessionRequest>,
	) -> Result<Response<proto::DesktopSessionResponse>, Status> {
		let request = request.into_inner();
		assert_eq!(request.browser_session_id, self.browser.to_string());
		assert_eq!(request.product_client_id, "fixture");
		assert!(Uuid::parse_str(&request.device_id).is_ok());
		assert_eq!(request.absolute_seconds, 7_776_000);
		assert_eq!(request.idle_seconds, 2_592_000);
		Ok(Response::new(proto::DesktopSessionResponse {
			product_session_id: self.desktop.to_string(),
			source_identity_id: self.source.to_string(),
			account_id: self.account.to_string(),
		}))
	}
}
struct RpcServer(tokio::task::JoinHandle<Result<(), tonic::transport::Error>>);
impl Drop for RpcServer {
	fn drop(&mut self) {
		self.0.abort();
	}
}

// The Cloud broker must save the newly issued common-auth Desktop session,
// rather than silently coupling its 90-day lifetime to the browser's 12 hours.
#[rstest::rstest]
#[tokio::test]
async fn common_auth_desktop_keeps_its_own_session_after_browser_logout_and_expiry(
	#[from(desktop_runtime)] runtime: common::RuntimeFuture,
) {
	let owner = runtime.await;
	let (mut f, url, schema) = owner.parts();
	let source: Uuid =
		sqlx::query_scalar("SELECT identity_id FROM dashboard_sessions WHERE NOT desktop")
			.fetch_one(f.store.pool.driver())
			.await
			.unwrap();
	let peer = AuthenticationPeer {
		source,
		account: Uuid::new_v4(),
		browser: Uuid::new_v4(),
		desktop: Uuid::new_v4(),
		browser_expired: Arc::new(false.into()),
		global_logout: Arc::new(false.into()),
	};
	let update = Query::update()
		.table(Alias::new("dashboard_sessions"))
		.value(Alias::new("provider_sid"), peer.browser.to_string())
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	let mut params = CertificateParams::default();
	params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
	let key = KeyPair::generate().unwrap();
	let ca = params.self_signed(&key).unwrap().pem();
	let issuer = Issuer::new(params, key);
	let leaf = |server| {
		let mut params = CertificateParams::new(vec!["authentication.internal".into()]).unwrap();
		params.extended_key_usages = vec![if server {
			ExtendedKeyUsagePurpose::ServerAuth
		} else {
			ExtendedKeyUsagePurpose::ClientAuth
		}];
		let key = KeyPair::generate().unwrap();
		(
			params.signed_by(&key, &issuer).unwrap().pem(),
			key.serialize_pem(),
		)
	};
	let (cert, key) = leaf(true);
	let mut server = GrpcServerTlsConfig::new(cert, key, ca.clone())
		.configure(tonic::transport::Server::builder())
		.unwrap();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("https://{}", listener.local_addr().unwrap());
	let _server = RpcServer(tokio::spawn(
		server
			.add_service(AuthenticationValidityServer::new(peer.clone()))
			.add_service(SessionRevocationServer::new(peer.clone()))
			.serve_with_incoming(TcpListenerStream::new(listener)),
	));
	let (cert, key) = leaf(false);
	f.config.common_auth = Some(Arc::new(
		Client::new(Settings {
			endpoint,
			server_domain: "authentication.internal".into(),
			ca_certificate: ca.into_bytes(),
			client_certificate: cert.into_bytes(),
			client_private_key: key.into_bytes(),
			product_client_id: "fixture".into(),
		})
		.unwrap(),
	));
	f.config.common_issuers = Some(
		aidash_server::apps::identity::serializers::settings::CommonAuthSettings {
			grpc_endpoint: "https://authentication.internal".into(),
			server_domain: "authentication.internal".into(),
			ca_certificate_file: "fixture-ca".into(),
			client_certificate_file: "fixture-client".into(),
			client_private_key_file: "fixture-key".into(),
			source_issuers: [("https://accounts.google.com".into(), String::new())].into(),
		},
	);
	f.store = f
		.store
		.clone()
		.with_dashboard_policy(f.config.dashboard_policy());
	let mut app = common::application(f.clone()).await;
	// The broker assertions observe each redirect, as desktop_fixture does.
	// A network client follows the anonymous redirect into the upstream login.
	let client = reinhardt::test::APIClient::from_handler(app.native_router());
	client.cleanup().await;
	app.api_http = Arc::new(client);
	let tokens = login(&app).await;
	let (saved, absolute): (String, i64) = sqlx::query_as("SELECT provider_sid, extract(epoch FROM expires_at-created_at)::bigint FROM dashboard_sessions WHERE desktop")
		.fetch_one(f.store.pool.driver()).await.unwrap();
	assert_eq!(saved, peer.desktop.to_string());
	assert_eq!(absolute, 7_776_000);
	assert_eq!(
		request(
			&app,
			"POST",
			"/auth/logout",
			&[
				("cookie", COOKIE),
				("origin", ORIGIN),
				("x-aidash-csrf", "desktop-csrf")
			],
			String::new()
		)
		.await
		.status(),
		StatusCode::NO_CONTENT
	);
	peer.browser_expired.store(true, Ordering::SeqCst);
	assert_eq!(
		bearer(&app, "GET", "/auth/session", &tokens["access_token"])
			.await
			.status(),
		StatusCode::OK
	);
	assert_eq!(
		bearer(&app, "POST", "/auth/logout-all", &tokens["access_token"])
			.await
			.status(),
		StatusCode::NO_CONTENT
	);
	assert!(peer.global_logout.load(Ordering::SeqCst));
	assert_eq!(
		bearer(&app, "GET", "/auth/session", &tokens["access_token"])
			.await
			.status(),
		StatusCode::UNAUTHORIZED
	);
	common::cleanup(f, &url, &schema).await;
}
