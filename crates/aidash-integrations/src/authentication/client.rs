use super::proto::{
	CreateDesktopSessionRequest, RestoreIdentityRequest, RevokeAllSessionsRequest,
	ValidateInteractiveRequest, ValidateOriginRequest, ValidityResponse,
	authentication_validity_client::AuthenticationValidityClient,
	session_revocation_client::SessionRevocationClient,
	source_lifecycle_client::SourceLifecycleClient,
};
use crate::{Error, Result};
use aidash_application::ports::accounts::Authority;
use async_trait::async_trait;
use reinhardt::grpc::{
	GrpcClientTlsConfig,
	tonic::{self, transport::Channel},
};
use std::time::Duration;
use uuid::Uuid;

/// TLS material is loaded by the process composition boundary, never from a request.
pub struct Settings {
	pub endpoint: String,
	pub server_domain: String,
	pub ca_certificate: Vec<u8>,
	pub client_certificate: Vec<u8>,
	pub client_private_key: Vec<u8>,
	pub product_client_id: String,
}

/// Cloning shares the authenticated HTTP/2 channel. No active result is cached.
#[derive(Clone)]
pub struct Client {
	channel: Channel,
	product_client_id: String,
}
fn rpc_error(status: tonic::Status) -> Error {
	match status.code() {
		tonic::Code::Unauthenticated => Error::Unauthorized,
		tonic::Code::PermissionDenied
		| tonic::Code::NotFound
		| tonic::Code::InvalidArgument
		| tonic::Code::FailedPrecondition => Error::Forbidden,
		_ => Error::IdentityStatusUnavailable,
	}
}
impl Client {
	pub async fn desktop_session(
		&self,
		browser: Uuid,
		source: Uuid,
		device: Uuid,
		absolute: u32,
		idle: u32,
	) -> Result<Uuid> {
		let response = SessionRevocationClient::new(self.channel.clone())
			.create_desktop_session(CreateDesktopSessionRequest {
				browser_session_id: browser.to_string(),
				product_client_id: self.product_client_id.clone(),
				device_id: device.to_string(),
				absolute_seconds: absolute,
				idle_seconds: idle,
			})
			.await
			.map_err(rpc_error)?
			.into_inner();
		if Uuid::parse_str(&response.source_identity_id).ok() != Some(source)
			|| Uuid::parse_str(&response.account_id).is_err()
		{
			return Err(Error::Forbidden);
		}
		Uuid::parse_str(&response.product_session_id).map_err(|_| Error::Forbidden)
	}
	pub async fn restore_identity(&self, source: Uuid, audit_reason: String) -> Result<()> {
		let result = SourceLifecycleClient::new(self.channel.clone())
			.restore_identity(RestoreIdentityRequest {
				external_identity_id: source.to_string(),
				audit_reason,
			})
			.await
			.map_err(rpc_error)?
			.into_inner();
		if result.completed {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	/// Explicit cross-product logout is distinct from a product-local cookie
	/// revocation. It requires this authenticated product session's account.
	pub async fn revoke_all_sessions(&self, session: Uuid) -> Result<()> {
		let response = SessionRevocationClient::new(self.channel.clone())
			.revoke_all_sessions(RevokeAllSessionsRequest {
				product_session_id: session.to_string(),
				product_client_id: self.product_client_id.clone(),
			})
			.await
			.map_err(rpc_error)?
			.into_inner();
		if response.completed {
			Ok(())
		} else {
			Err(Error::IdentityStatusUnavailable)
		}
	}
	pub fn new(settings: Settings) -> Result<Self> {
		if settings.product_client_id.trim().is_empty() {
			return Err(Error::Invalid(
				"common authentication client id is required".into(),
			));
		}
		let tls = GrpcClientTlsConfig::new(
			settings.ca_certificate,
			settings.client_certificate,
			settings.client_private_key,
			settings.server_domain,
		);
		let channel = tls
			.endpoint(&settings.endpoint)
			.map_err(|_| Error::Invalid("invalid common authentication mTLS endpoint".into()))?
			.connect_timeout(Duration::from_secs(3))
			.timeout(Duration::from_secs(3))
			.connect_lazy();
		Ok(Self {
			channel,
			product_client_id: settings.product_client_id,
		})
	}
	pub async fn interactive(&self, identity: Uuid, session: Uuid) -> Result<ValidityResponse> {
		let response = AuthenticationValidityClient::new(self.channel.clone())
			.validate_interactive(ValidateInteractiveRequest {
				external_identity_id: identity.to_string(),
				product_session_id: session.to_string(),
				product_client_id: self.product_client_id.clone(),
			})
			.await
			.map_err(rpc_error)?
			.into_inner();
		require_validity(identity, &response, true)?;
		Ok(response)
	}
}

fn require_validity(identity: Uuid, response: &ValidityResponse, interactive: bool) -> Result<()> {
	if !response.active {
		return if response.reason == "unavailable" {
			Err(Error::IdentityStatusUnavailable)
		} else {
			Err(Error::Forbidden)
		};
	}
	if Uuid::parse_str(&response.source_identity_id).ok() != Some(identity)
		|| ((interactive || !response.account_id.is_empty())
			&& Uuid::parse_str(&response.account_id).is_err())
		|| (!interactive && !response.account_id.is_empty() && response.suspension_epoch.is_none())
	{
		return Err(Error::Forbidden);
	}
	Ok(())
}

#[async_trait]
impl Authority for Client {
	async fn validate_interactive(
		&self,
		source_identity: Uuid,
		product_session: Uuid,
	) -> Result<()> {
		self.interactive(source_identity, product_session)
			.await
			.map(|_| ())
	}
	async fn validate_origin(&self, source_identity: Uuid) -> Result<()> {
		self.origin_binding(source_identity).await.map(|_| ())
	}
	async fn origin_binding(
		&self,
		source_identity: Uuid,
	) -> Result<aidash_domain::accounts::DurableOrigin> {
		let response = AuthenticationValidityClient::new(self.channel.clone())
			.validate_origin(ValidateOriginRequest {
				external_identity_id: source_identity.to_string(),
				expected_account_id: String::new(),
				expected_suspension_epoch: None,
				expected_source_disable_epoch: None,
				bind_current: true,
			})
			.await
			.map_err(rpc_error)?
			.into_inner();
		require_validity(source_identity, &response, false)?;
		Ok(aidash_domain::accounts::DurableOrigin {
			identity_id: source_identity,
			account_id: if response.account_id.is_empty() {
				None
			} else {
				Some(Uuid::parse_str(&response.account_id).map_err(|_| Error::Forbidden)?)
			},
			suspension_epoch: response.suspension_epoch,
			source_disable_epoch: Some(response.source_disable_epoch),
		})
	}
	async fn validate_bound_origin(
		&self,
		origin: &aidash_domain::accounts::DurableOrigin,
	) -> Result<()> {
		let response = AuthenticationValidityClient::new(self.channel.clone())
			.validate_origin(ValidateOriginRequest {
				external_identity_id: origin.identity_id.to_string(),
				expected_account_id: origin
					.account_id
					.map(|id| id.to_string())
					.unwrap_or_default(),
				expected_suspension_epoch: origin.suspension_epoch,
				expected_source_disable_epoch: origin.source_disable_epoch,
				bind_current: false,
			})
			.await
			.map_err(rpc_error)?
			.into_inner();
		require_validity(origin.identity_id, &response, false)?;
		if origin
			.account_id
			.is_some_and(|id| response.account_id != id.to_string())
			|| origin.suspension_epoch.unwrap_or(0) != response.suspension_epoch.unwrap_or(0)
			|| origin.source_disable_epoch.unwrap_or(0) != response.source_disable_epoch
		{
			return Err(Error::Forbidden);
		}
		Ok(())
	}
}
