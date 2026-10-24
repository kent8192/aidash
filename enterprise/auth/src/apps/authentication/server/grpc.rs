//! Private mTLS service adapters; product peers never obtain administrator scope.
use crate::{Error, Result, service::Authentication};
use aidash_application::{accounts::Service as Accounts, ports::accounts::Scope};
use aidash_domain::accounts::{Administrator, Validity};
use aidash_integrations::authentication::proto::{
	self,
	account_administration_server::{AccountAdministration, AccountAdministrationServer},
	authentication_validity_server::{AuthenticationValidity, AuthenticationValidityServer},
	session_revocation_server::{SessionRevocation, SessionRevocationServer},
	source_lifecycle_server::{SourceLifecycle, SourceLifecycleServer},
};
use reinhardt::grpc::{
	GrpcServerTlsConfig, PeerIdentity, PeerIdentityInterceptor,
	tonic::{self, Request, Response, Status},
};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;

#[derive(Clone)]
pub struct Rpc {
	auth: Arc<Authentication>,
}
impl Rpc {
	pub fn new(auth: Arc<Authentication>) -> Self {
		Self { auth }
	}
	fn product<T>(&self, request: &Request<T>) -> std::result::Result<String, Status> {
		let peer = request
			.extensions()
			.get::<PeerIdentity>()
			.ok_or_else(|| Status::unauthenticated("authenticated service identity required"))?;
		self.auth
			.config
			.grpc
			.product_peers
			.iter()
			.find(|(_, uri)| peer.uri_sans().contains(uri))
			.map(|(client, _)| client.clone())
			.ok_or_else(|| Status::permission_denied("product service required"))
	}
	async fn owns_session(&self, session: &str, client: &str) -> std::result::Result<Uuid, Status> {
		let id = uuid(session)?;
		let mut tx = self.auth.store.scope().await.map_err(status)?;
		let session = tx
			.session(id)
			.await
			.map_err(status)?
			.filter(|s| s.client_id == client)
			.ok_or_else(|| Status::permission_denied("product session differs"))?;
		Ok(session.id)
	}
}
#[tonic::async_trait]
impl AuthenticationValidity for Rpc {
	async fn validate_interactive(
		&self,
		request: Request<proto::ValidateInteractiveRequest>,
	) -> std::result::Result<Response<proto::ValidityResponse>, Status> {
		let client = self.product(&request)?;
		let input = request.into_inner();
		if input.product_client_id != client {
			return Err(Status::permission_denied("product client differs"));
		}
		let session = self
			.owns_session(&input.product_session_id, &client)
			.await?;
		let source_id = uuid(&input.external_identity_id)?;
		let (validity, session, source) = self.auth.check_session(session).await.map_err(status)?;
		let session = session.ok_or_else(|| Status::unauthenticated("product session invalid"))?;
		if session.identity_id != source_id {
			return Err(Status::permission_denied("authentication source differs"));
		}
		let mut response = validity_response(validity, source);
		response.account_id = session.account_id.to_string();
		Ok(Response::new(response))
	}
	async fn validate_origin(
		&self,
		request: Request<proto::ValidateOriginRequest>,
	) -> std::result::Result<Response<proto::ValidityResponse>, Status> {
		self.product(&request)?;
		let input = request.into_inner();
		let source = uuid(&input.external_identity_id)?;
		let expected = if input.expected_account_id.is_empty() {
			None
		} else {
			Some(uuid(&input.expected_account_id)?)
		};
		let (validity, source, epoch) = self
			.auth
			.check_origin(
				source,
				expected,
				input.expected_suspension_epoch,
				input.expected_source_disable_epoch,
				input.bind_current,
			)
			.await
			.map_err(status)?;
		let mut response = validity_response(validity, source);
		response.suspension_epoch = epoch;
		Ok(Response::new(response))
	}
}
#[tonic::async_trait]
impl SessionRevocation for Rpc {
	async fn create_desktop_session(
		&self,
		request: Request<proto::CreateDesktopSessionRequest>,
	) -> std::result::Result<Response<proto::DesktopSessionResponse>, Status> {
		let client = self.product(&request)?;
		let input = request.into_inner();
		if input.product_client_id != client || client != "cloud" {
			return Err(Status::permission_denied("Cloud product client required"));
		}
		let parent = self
			.owns_session(&input.browser_session_id, &client)
			.await?;
		let session = self
			.auth
			.create_desktop_session(
				parent,
				&client,
				uuid(&input.device_id)?,
				input.absolute_seconds,
				input.idle_seconds,
			)
			.await
			.map_err(status)?;
		Ok(Response::new(proto::DesktopSessionResponse {
			product_session_id: session.id.to_string(),
			source_identity_id: session.identity_id.to_string(),
			account_id: session.account_id.to_string(),
		}))
	}

	async fn revoke_product_session(
		&self,
		request: Request<proto::RevokeProductSessionRequest>,
	) -> std::result::Result<Response<proto::RevocationResponse>, Status> {
		let client = self.product(&request)?;
		let input = request.into_inner();
		if input.product_client_id != client {
			return Err(Status::permission_denied("product client differs"));
		}
		let id = self
			.owns_session(&input.product_session_id, &client)
			.await?;
		Accounts::new(self.auth.store.clone())
			.logout(id, chrono::Utc::now())
			.await
			.map_err(|_| Status::unavailable("session revocation unavailable"))?;
		Ok(Response::new(proto::RevocationResponse { completed: true }))
	}
	async fn revoke_all_sessions(
		&self,
		request: Request<proto::RevokeAllSessionsRequest>,
	) -> std::result::Result<Response<proto::RevocationResponse>, Status> {
		let client = self.product(&request)?;
		let input = request.into_inner();
		if input.product_client_id != client {
			return Err(Status::permission_denied("product client differs"));
		}
		let id = self
			.owns_session(&input.product_session_id, &client)
			.await?;
		let (validity, _, _) = self.auth.check_session(id).await.map_err(status)?;
		if validity != Validity::Active {
			return Err(Status::unauthenticated("active product session required"));
		}
		Accounts::new(self.auth.store.clone())
			.logout_all_authenticated(id, chrono::Utc::now())
			.await
			.map_err(|_| Status::unavailable("session revocation unavailable"))?;
		Ok(Response::new(proto::RevocationResponse { completed: true }))
	}
}
#[tonic::async_trait]
impl AccountAdministration for Rpc {
	async fn suspend_account(
		&self,
		request: Request<proto::AccountAdministrationRequest>,
	) -> std::result::Result<Response<proto::RevocationResponse>, Status> {
		let peer = self.admin_peer(&request)?;
		let input = request.into_inner();
		let account = uuid(&input.account_id)?;
		require_audit_reason(&input.audit_reason)?;
		self.audit(account, &peer, "suspend_requested", &input.audit_reason)
			.await
			.map_err(status)?;
		Accounts::new(self.auth.store.clone())
			.suspend(Administrator::Authentication, account, chrono::Utc::now())
			.await
			.map_err(|_| Status::unavailable("account suspension unavailable"))?;
		Ok(Response::new(proto::RevocationResponse { completed: true }))
	}
	async fn restore_account(
		&self,
		request: Request<proto::AccountAdministrationRequest>,
	) -> std::result::Result<Response<proto::RevocationResponse>, Status> {
		let peer = self.admin_peer(&request)?;
		let input = request.into_inner();
		let account = uuid(&input.account_id)?;
		require_audit_reason(&input.audit_reason)?;
		self.audit(account, &peer, "restore_requested", &input.audit_reason)
			.await
			.map_err(status)?;
		Accounts::new(self.auth.store.clone())
			.restore(Administrator::Authentication, account)
			.await
			.map_err(|_| Status::unavailable("account restoration unavailable"))?;
		Ok(Response::new(proto::RevocationResponse { completed: true }))
	}
}
#[tonic::async_trait]
impl SourceLifecycle for Rpc {
	async fn restore_identity(
		&self,
		request: Request<proto::RestoreIdentityRequest>,
	) -> std::result::Result<Response<proto::RevocationResponse>, Status> {
		let client = self.product(&request)?;
		if !self
			.auth
			.config
			.grpc
			.source_restore_clients
			.contains(&client)
		{
			return Err(Status::permission_denied(
				"source restoration service permission required",
			));
		}
		let input = request.into_inner();
		let source_id = uuid(&input.external_identity_id)?;
		require_audit_reason(&input.audit_reason)?;
		let mut tx = self.auth.store.scope().await.map_err(status)?;
		let identity = tx
			.identity(source_id)
			.await
			.map_err(status)?
			.ok_or_else(|| Status::not_found("source identity not found"))?;
		drop(tx);
		let state = self
			.auth
			.upstream
			.lookup(&identity.issuer, &identity.subject)
			.await
			.map_err(status)?;
		if state.disabled {
			return Err(Status::failed_precondition(
				"source remains disabled upstream",
			));
		}
		self.audit(
			source_id,
			&client,
			"source_restore_requested",
			&input.audit_reason,
		)
		.await
		.map_err(status)?;
		let mut tx = self.auth.store.scope().await.map_err(status)?;
		let mut identity = tx
			.identity(source_id)
			.await
			.map_err(status)?
			.ok_or_else(|| Status::not_found("source identity not found"))?;
		if identity.status == aidash_domain::accounts::IdentityStatus::Disabled {
			tx.revoke_identity_sessions(source_id, chrono::Utc::now())
				.await
				.map_err(status)?;
			identity.status = aidash_domain::accounts::IdentityStatus::Enabled;
		}
		identity.last_valid_at = Some(chrono::Utc::now());
		// Preserve the stop epoch: old origins still need explicit manual resume.
		tx.observe_source(source_id, state.valid_since)
			.await
			.map_err(status)?;
		tx.save_identity(&identity).await.map_err(status)?;
		Box::new(tx).commit().await.map_err(status)?;
		Ok(Response::new(proto::RevocationResponse { completed: true }))
	}
}

impl Rpc {
	fn admin_peer<T>(&self, request: &Request<T>) -> std::result::Result<String, Status> {
		let peer = request
			.extensions()
			.get::<PeerIdentity>()
			.ok_or_else(|| Status::unauthenticated("authenticated administrator required"))?;
		peer.uri_sans()
			.iter()
			.find(|uri| self.auth.config.grpc.administrator_peers.contains(uri))
			.cloned()
			.ok_or_else(|| Status::permission_denied("authentication administrator required"))
	}
	async fn audit(&self, account: Uuid, peer: &str, action: &str, reason: &str) -> Result<()> {
		let id = Uuid::new_v4();
		let mut tx = self.auth.store.scope().await?;
		// Retain the authenticated administrator and intent before executing a
		// stop. A failed command can be retried; absence of its result is never
		// represented as a successful state change by the audit record.
		tx.save_flow(id, "administration-audit", &serde_json::json!({"account_id":account,"peer":peer,"action":action,"reason":reason,"requested_at":chrono::Utc::now()})).await?;
		Box::new(tx).commit().await?;
		Ok(())
	}
}
fn require_audit_reason(reason: &str) -> std::result::Result<(), Status> {
	if reason.trim().is_empty() || reason.len() > 512 {
		return Err(Status::invalid_argument("bounded audit reason required"));
	}
	Ok(())
}
fn uuid(value: &str) -> std::result::Result<Uuid, Status> {
	Uuid::parse_str(value).map_err(|_| Status::invalid_argument("UUID required"))
}
fn status(_: impl std::fmt::Display) -> Status {
	Status::unavailable("authentication authority unavailable")
}
fn validity_response(
	validity: Validity,
	source: Option<aidash_domain::accounts::ExternalIdentity>,
) -> proto::ValidityResponse {
	let (active, reason) = match validity {
		Validity::Active => (true, String::new()),
		Validity::Unavailable => (false, "unavailable".into()),
		Validity::Denied(reason) => (false, format!("{reason:?}").to_ascii_lowercase()),
	};
	let mut result = proto::ValidityResponse {
		active,
		reason,
		source_identity_id: String::new(),
		account_id: String::new(),
		source_issuer: String::new(),
		source_subject: String::new(),
		suspension_epoch: None,
		source_disable_epoch: 0,
	};
	if let Some(source) = source {
		result.source_disable_epoch = source.disable_epoch;
		result.source_identity_id = source.id.to_string();
		result.account_id = source
			.first_account
			.map(|v| v.to_string())
			.unwrap_or_default();
		result.source_issuer = source.issuer;
		result.source_subject = source.subject;
	}
	result
}
/// Serve only authenticated peers; the separate generated services carry separate
/// policies. No header, common name or caller-supplied role selects a permission.
pub async fn serve(auth: Arc<Authentication>) -> Result<()> {
	let listener = tokio::net::TcpListener::bind(auth.config.grpc.listen)
		.await
		.map_err(|_| Error::Unavailable)?;
	serve_on(auth, listener).await
}
pub(crate) async fn serve_on(
	auth: Arc<Authentication>,
	listener: tokio::net::TcpListener,
) -> Result<()> {
	let config = auth.config.grpc.clone();
	let tls = GrpcServerTlsConfig::new(
		std::fs::read(&config.certificate_file).map_err(|_| Error::Configuration)?,
		std::fs::read(&config.private_key_file).map_err(|_| Error::Configuration)?,
		std::fs::read(&config.client_ca_file).map_err(|_| Error::Configuration)?,
	);
	let peers = config.product_peers.clone();
	let product = move |peer: &PeerIdentity| authorize_product(&peers, peer);
	let peers = config.product_peers.clone();
	let revocation = move |peer: &PeerIdentity| authorize_product(&peers, peer);
	let admins = config.administrator_peers.clone();
	let administration = move |peer: &PeerIdentity| {
		if peer.uri_sans().iter().any(|uri| admins.contains(uri)) {
			Ok(())
		} else {
			Err(Status::permission_denied(
				"authentication administrator required",
			))
		}
	};
	let restore_peers: Vec<String> = config
		.source_restore_clients
		.iter()
		.filter_map(|client| config.product_peers.get(client).cloned())
		.collect();
	let restoration = move |peer: &PeerIdentity| {
		if peer
			.uri_sans()
			.iter()
			.any(|uri| restore_peers.contains(uri))
		{
			Ok(())
		} else {
			Err(Status::permission_denied(
				"source restoration service permission required",
			))
		}
	};
	let rpc = Rpc::new(auth);
	tls.configure(tonic::transport::Server::builder())
		.map_err(|_| Error::Configuration)?
		.add_service(AuthenticationValidityServer::with_interceptor(
			rpc.clone(),
			PeerIdentityInterceptor::new(product),
		))
		.add_service(SessionRevocationServer::with_interceptor(
			rpc.clone(),
			PeerIdentityInterceptor::new(revocation),
		))
		.add_service(AccountAdministrationServer::with_interceptor(
			rpc.clone(),
			PeerIdentityInterceptor::new(administration),
		))
		.add_service(SourceLifecycleServer::with_interceptor(
			rpc,
			PeerIdentityInterceptor::new(restoration),
		))
		.serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
		.await
		.map_err(|_| Error::Unavailable)?;
	Ok(())
}
fn authorize_product(
	peers: &BTreeMap<String, String>,
	peer: &PeerIdentity,
) -> std::result::Result<(), Status> {
	if peers.values().any(|uri| peer.uri_sans().contains(uri)) {
		Ok(())
	} else {
		Err(Status::permission_denied("product service required"))
	}
}
