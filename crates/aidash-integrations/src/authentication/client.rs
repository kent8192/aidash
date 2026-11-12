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
use std::{
	collections::HashMap,
	hash::Hash,
	sync::{Arc, Mutex},
	time::{Duration, Instant},
};
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

type OriginKey = (Uuid, Option<Uuid>, Option<u64>, Option<u64>);
type Entry = Arc<tokio::sync::Mutex<Option<Instant>>>;
type Cache<K> = Arc<Mutex<HashMap<K, Entry>>>;

fn entry<K: Eq + Hash>(cache: &Cache<K>, key: K, ttl: Duration) -> Entry {
	let now = Instant::now();
	let mut cache = cache.lock().expect("validity cache");
	cache.retain(|_, value| {
		Arc::strong_count(value) > 1
			|| value
				.try_lock()
				.map_or(true, |at| at.is_some_and(|at| now.duration_since(at) < ttl))
	});
	cache
		.entry(key)
		.or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
		.clone()
}

/// Clones share the channel and the cache for admitted work.
#[derive(Clone)]
pub struct Client {
	channel: Channel,
	product_client_id: String,
	cache_ttl: Duration,
	origin_cache: Cache<OriginKey>,
	interactive_cache: Cache<(Uuid, Uuid)>,
	/// The dashboard reads its session on every page load; the sign-in method
	/// count is asked at most once per session per [`SIGN_IN_METHOD_COUNT_TTL`].
	sign_in_method_counts: Arc<Mutex<HashMap<(Uuid, Uuid), (Instant, Option<u32>)>>>,
}

const SIGN_IN_METHOD_COUNT_TTL: Duration = Duration::from_secs(60 * 60);
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
			cache_ttl: Duration::ZERO,
			origin_cache: Arc::new(Mutex::new(HashMap::new())),
			interactive_cache: Arc::new(Mutex::new(HashMap::new())),
			sign_in_method_counts: Arc::new(Mutex::new(HashMap::new())),
		})
	}
	pub fn with_validity_cache_seconds(mut self, seconds: u64) -> Self {
		self.cache_ttl = Duration::from_secs(seconds);
		self
	}
	pub async fn interactive(&self, identity: Uuid, session: Uuid) -> Result<ValidityResponse> {
		let entry = entry(&self.interactive_cache, (identity, session), self.cache_ttl);
		let mut last = entry.lock().await;
		let checked = Instant::now();
		let response = match self.interactive_online(identity, session).await {
			Ok(response) => response,
			// A definite refusal ends the reuse at once, so open streams of the
			// session stop at their next frame instead of up to T later.
			Err(error @ (Error::Forbidden | Error::Unauthorized)) => {
				*last = None;
				return Err(error);
			}
			Err(error) => return Err(error),
		};
		if !self.cache_ttl.is_zero() {
			*last = Some(checked);
		}
		Ok(response)
	}
	pub async fn forget_session(&self, identity: Uuid, session: Uuid) {
		let removed = self
			.interactive_cache
			.lock()
			.expect("validity cache")
			.remove(&(identity, session));
		if let Some(entry) = removed {
			*entry.lock().await = None;
		}
	}
	pub async fn forget_identity(&self, identity: Uuid) {
		let removed = {
			let mut cache = self.interactive_cache.lock().expect("validity cache");
			let keys: Vec<_> = cache
				.keys()
				.copied()
				.filter(|(id, _)| *id == identity)
				.collect();
			keys.into_iter()
				.filter_map(|key| cache.remove(&key))
				.collect::<Vec<_>>()
		};
		for entry in removed {
			*entry.lock().await = None;
		}
	}
	async fn interactive_online(&self, identity: Uuid, session: Uuid) -> Result<ValidityResponse> {
		self.interactive_read(identity, session, false).await
	}
	pub async fn sign_in_method_count(&self, identity: Uuid, session: Uuid) -> Result<Option<u32>> {
		let now = Instant::now();
		if let Some((at, count)) = self
			.sign_in_method_counts
			.lock()
			.expect("sign-in method counts")
			.get(&(identity, session))
			&& now.duration_since(*at) < SIGN_IN_METHOD_COUNT_TTL
		{
			return Ok(*count);
		}
		let count = self
			.interactive_read(identity, session, true)
			.await?
			.usable_sign_in_method_count;
		let mut counts = self
			.sign_in_method_counts
			.lock()
			.expect("sign-in method counts");
		counts.retain(|_, (at, _)| now.duration_since(*at) < SIGN_IN_METHOD_COUNT_TTL);
		counts.insert((identity, session), (now, count));
		Ok(count)
	}
	async fn interactive_read(
		&self,
		identity: Uuid,
		session: Uuid,
		include_sign_in_method_count: bool,
	) -> Result<ValidityResponse> {
		let response = AuthenticationValidityClient::new(self.channel.clone())
			.validate_interactive(ValidateInteractiveRequest {
				external_identity_id: identity.to_string(),
				product_session_id: session.to_string(),
				product_client_id: self.product_client_id.clone(),
				include_sign_in_method_count,
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
	async fn validate_interactive_within_t(
		&self,
		source_identity: Uuid,
		product_session: Uuid,
	) -> Result<()> {
		if self.cache_ttl.is_zero() {
			return self
				.interactive(source_identity, product_session)
				.await
				.map(|_| ());
		}
		let entry = entry(
			&self.interactive_cache,
			(source_identity, product_session),
			self.cache_ttl,
		);
		let mut last = entry.lock().await;
		let checked = Instant::now();
		if last.is_some_and(|at| checked.duration_since(at) < self.cache_ttl) {
			return Ok(());
		}
		self.interactive_online(source_identity, product_session)
			.await?;
		*last = Some(checked);
		Ok(())
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
		if !response.active
			&& response.reason == "account_suspended"
			&& Uuid::parse_str(&response.source_identity_id).ok() == Some(origin.identity_id)
			&& origin
				.account_id
				.is_none_or(|id| response.account_id == id.to_string())
			&& origin.source_disable_epoch.unwrap_or(0) == response.source_disable_epoch
		{
			if let Some(epoch) = response.suspension_epoch {
				return Err(Error::AccountSuspended { epoch });
			}
		}
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
	async fn validate_bound_origin_within_t(
		&self,
		origin: &aidash_domain::accounts::DurableOrigin,
	) -> Result<()> {
		if self.cache_ttl.is_zero() {
			return self.validate_bound_origin(origin).await;
		}
		let key = (
			origin.identity_id,
			origin.account_id,
			origin.suspension_epoch,
			origin.source_disable_epoch,
		);
		let entry = entry(&self.origin_cache, key, self.cache_ttl);
		let mut last = entry.lock().await;
		let now = Instant::now();
		if last.is_some_and(|checked| now.duration_since(checked) < self.cache_ttl) {
			return Ok(());
		}
		self.validate_bound_origin(origin).await?;
		*last = Some(now);
		Ok(())
	}
	async fn resume_suspended_origin(
		&self,
		origin: &aidash_domain::accounts::DurableOrigin,
		epoch: u64,
	) -> Result<Option<aidash_domain::accounts::DurableOrigin>> {
		let response = AuthenticationValidityClient::new(self.channel.clone())
			.validate_origin(ValidateOriginRequest {
				external_identity_id: origin.identity_id.to_string(),
				expected_account_id: origin
					.account_id
					.map(|id| id.to_string())
					.unwrap_or_default(),
				expected_suspension_epoch: origin.suspension_epoch,
				expected_source_disable_epoch: origin.source_disable_epoch,
				bind_current: true,
			})
			.await
			.map_err(rpc_error)?
			.into_inner();
		require_validity(origin.identity_id, &response, false)?;
		if response.suspension_epoch != Some(epoch)
			|| !response.resumable_suspension_epochs.contains(&epoch)
			|| origin
				.account_id
				.is_some_and(|id| response.account_id != id.to_string())
			|| origin.source_disable_epoch.unwrap_or(0) != response.source_disable_epoch
		{
			return Ok(None);
		}
		let account_id = Uuid::parse_str(&response.account_id).map_err(|_| Error::Forbidden)?;
		Ok(Some(aidash_domain::accounts::DurableOrigin {
			identity_id: origin.identity_id,
			account_id: Some(account_id),
			suspension_epoch: Some(epoch),
			source_disable_epoch: Some(response.source_disable_epoch),
		}))
	}
}
