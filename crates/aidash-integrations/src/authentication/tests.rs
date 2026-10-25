//! The adapter must perform new online checks and preserve source provenance.
use super::{
	Client, Settings,
	proto::{
		ValidateInteractiveRequest, ValidateOriginRequest, ValidityResponse,
		authentication_validity_server::{AuthenticationValidity, AuthenticationValidityServer},
	},
};
use aidash_application::{Error, ports::accounts::Authority};
use rcgen::{BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair};
use reinhardt::grpc::{
	GrpcServerTlsConfig,
	tonic::{self, Request, Response, Status},
};
use std::sync::{Arc, Mutex};
use tokio_stream::wrappers::TcpListenerStream;
use uuid::Uuid;

struct Peer {
	response: Arc<Mutex<ValidityResponse>>,
	calls: Arc<Mutex<Vec<(String, String)>>>,
}
#[tonic::async_trait]
impl AuthenticationValidity for Peer {
	async fn validate_interactive(
		&self,
		request: Request<ValidateInteractiveRequest>,
	) -> Result<Response<ValidityResponse>, Status> {
		let request = request.into_inner();
		assert_eq!(request.product_client_id, "cloud-client");
		self.calls
			.lock()
			.unwrap()
			.push((request.external_identity_id, request.product_session_id));
		let mut response = self.response.lock().unwrap().clone();
		// Interactive admission checks a ProductSession, not a durable origin;
		// the real RPC need not return a saved Run suspension epoch here.
		response.suspension_epoch = None;
		Ok(Response::new(response))
	}
	async fn validate_origin(
		&self,
		request: Request<ValidateOriginRequest>,
	) -> Result<Response<ValidityResponse>, Status> {
		let request = request.into_inner();
		self.calls
			.lock()
			.unwrap()
			.push((request.external_identity_id, String::new()));
		Ok(Response::new(self.response.lock().unwrap().clone()))
	}
}

struct Running {
	task: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
	stop: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Running {
	async fn shutdown(&mut self) {
		self.stop.take().unwrap().send(()).unwrap();
		(&mut self.task).await.unwrap().unwrap();
	}
}
impl Drop for Running {
	fn drop(&mut self) {
		self.task.abort();
	}
}

// The real generated RPC + mTLS transport catches positive-result caching,
// source/session substitution and unbounded transport failures independently
// from the authentication server's lifecycle rules.
#[tokio::test]
async fn fresh_status_and_original_provenance_are_required_over_mtls() {
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
	let source = Uuid::new_v4();
	let session = Uuid::new_v4();
	let response = Arc::new(Mutex::new(ValidityResponse {
		active: true,
		reason: String::new(),
		source_identity_id: source.to_string(),
		account_id: Uuid::new_v4().to_string(),
		suspension_epoch: Some(0),
		source_disable_epoch: 0,
		source_issuer: "upstream".into(),
		source_subject: "source".into(),
	}));
	let calls = Arc::new(Mutex::new(Vec::new()));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("https://{}", listener.local_addr().unwrap());
	let (cert, key) = leaf(true);
	let mut server = GrpcServerTlsConfig::new(cert, key, ca.clone())
		.configure(tonic::transport::Server::builder())
		.unwrap();
	let (stop, stopped) = tokio::sync::oneshot::channel();
	let mut running = Running {
		task: tokio::spawn(
			server
				.add_service(AuthenticationValidityServer::new(Peer {
					response: response.clone(),
					calls: calls.clone(),
				}))
				.serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
					let _ = stopped.await;
				}),
		),
		stop: Some(stop),
	};
	let (cert, key) = leaf(false);
	let client = Client::new(Settings {
		endpoint,
		server_domain: "authentication.internal".into(),
		ca_certificate: ca.into_bytes(),
		client_certificate: cert.into_bytes(),
		client_private_key: key.into_bytes(),
		product_client_id: "cloud-client".into(),
	})
	.unwrap();
	client.validate_interactive(source, session).await.unwrap();
	client.validate_origin(source).await.unwrap();
	let admitted = client.origin_binding(source).await.unwrap();
	assert_eq!(admitted.suspension_epoch, Some(0));
	let legacy = aidash_domain::accounts::DurableOrigin {
		identity_id: source,
		account_id: None,
		suspension_epoch: None,
		source_disable_epoch: None,
	};
	// First Account assignment may bind old work only before any stop epoch.
	client.validate_bound_origin(&legacy).await.unwrap();
	response.lock().unwrap().suspension_epoch = Some(1);
	// Restoring an account never refreshes an old Run's saved admission epoch.
	assert!(matches!(
		client.validate_bound_origin(&admitted).await,
		Err(Error::Forbidden)
	));
	assert!(matches!(
		client.validate_bound_origin(&legacy).await,
		Err(Error::Forbidden)
	));
	let manually_readmitted = client.origin_binding(source).await.unwrap();
	client
		.validate_bound_origin(&manually_readmitted)
		.await
		.unwrap();
	response.lock().unwrap().suspension_epoch = Some(0);
	response.lock().unwrap().active = false;
	assert!(matches!(
		client.validate_interactive(source, session).await,
		Err(Error::Forbidden)
	));
	assert!(matches!(
		client.validate_origin(source).await,
		Err(Error::Forbidden)
	));
	{
		let mut response = response.lock().unwrap();
		response.active = false;
		response.reason = "unavailable".into();
	}
	assert!(matches!(
		client.validate_origin(source).await,
		Err(Error::IdentityStatusUnavailable)
	));
	{
		let mut response = response.lock().unwrap();
		response.active = true;
		response.source_identity_id = source.to_string();
		response.account_id.clear();
		response.reason.clear();
	}
	// Legacy admitted work survives without eagerly allocating a common Account.
	client.validate_origin(source).await.unwrap();
	assert!(matches!(
		client.validate_interactive(source, session).await,
		Err(Error::Forbidden)
	));
	{
		let mut response = response.lock().unwrap();
		response.active = true;
		response.source_identity_id = Uuid::new_v4().to_string();
	}
	assert!(matches!(
		client.validate_origin(source).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		&calls.lock().unwrap()[..2],
		&[
			(source.to_string(), session.to_string()),
			(source.to_string(), String::new()),
		]
	);
	running.shutdown().await;
	assert!(matches!(
		client.validate_origin(source).await,
		Err(Error::IdentityStatusUnavailable)
	));
}
