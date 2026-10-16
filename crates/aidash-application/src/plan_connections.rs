//! Plan Connection lifecycle (ADR 0007, ADR 0031). The server runs Sign in
//! with ChatGPT: it keeps the `state`, the `nonce` and the PKCE verifier, and
//! it alone exchanges the code. The callback URL reaches it from a desktop
//! relay or a paste. Plan Tokens go straight into the Provider Credential
//! Store; only a narrow read port returns them, for one call.
use crate::{
	Error, Result,
	provider_access::KeyMaterialReader,
	provider_authorization::{challenge, random_secret},
	provider_credentials::Store,
};
use aidash_domain::plan_connections::{
	self as domain, Callback, CallbackResult, Failure, PlanConnection, PlanModel, State,
};
use aidash_domain::provider_credentials::Provider;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// How long a pending authorization waits for its callback URL. It covers
/// the consent page and, for a paste, copying the address back.
pub const LIFETIME_MINUTES: i64 = 15;
/// The actor recorded for writes no person made, such as a call's refresh.
pub const SYSTEM_ACTOR: &str = "system";

/// The subject that owns a Plan Connection, in its Tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
	pub tenant: String,
	pub subject: String,
}

/// The host-held half of one authorization. The browser or desktop client
/// receives only `url`.
pub struct Request {
	pub state: SecretString,
	pub nonce: SecretString,
	pub verifier: SecretString,
	pub redirect_uri: String,
	pub url: String,
}

/// Starts one authorization for a loopback `port`: the desktop client's
/// listener, or [`domain::PASTE_PORT`] for a paste. A reconnect reuses the
/// connection's issued client, so OpenAI keeps its registration and limits.
pub fn begin(host_id: &str, port: u16, saved_client: Option<&str>) -> Result<Request> {
	let redirect_uri = domain::redirect_uri(port)?;
	let state = SecretString::from(random_secret());
	let nonce = SecretString::from(random_secret());
	let verifier = SecretString::from(random_secret());
	let mut url = url::Url::parse(Provider::Chatgpt.authorization_url())
		.map_err(|_| Error::Invalid("invalid Sign in with ChatGPT URL".into()))?;
	{
		let mut query = url.query_pairs_mut();
		query
			.append_pair("response_type", "code")
			.append_pair("client_id", saved_client.unwrap_or(domain::DYNAMIC_CLIENT))
			.append_pair("redirect_uri", &redirect_uri)
			.append_pair("scope", domain::SCOPES)
			.append_pair("resource", domain::RESOURCE)
			.append_pair("state", state.expose_secret())
			.append_pair("nonce", nonce.expose_secret())
			.append_pair("code_challenge", &challenge(&verifier))
			.append_pair("code_challenge_method", "S256")
			.append_pair("ext_agent_host_id", host_id);
		if saved_client.is_none() {
			query.append_pair("agent_name_hint", domain::AGENT_NAME);
		}
	}
	Ok(Request {
		state,
		nonce,
		verifier,
		redirect_uri,
		url: url.into(),
	})
}

/// Tokens OpenAI issued from a code exchange or a refresh.
pub struct Issued {
	pub access_token: SecretString,
	pub refresh_token: SecretString,
	pub id_token: Option<SecretString>,
	pub expires_in: i64,
	/// Granted scopes; a refresh may omit them, which keeps the grant.
	pub scope: Option<String>,
	/// OpenAI's `earliest_refresh_at`: no refresh before it.
	pub earliest_refresh_at: Option<DateTime<Utc>>,
}

pub struct ExchangeRequest<'a> {
	pub client_id: &'a str,
	pub code: &'a str,
	pub verifier: &'a SecretString,
	pub redirect_uri: &'a str,
}

/// Why a refresh failed.
#[derive(Debug)]
pub enum RefreshFailure {
	/// The refresh token is unusable: the grant has ended.
	Terminal,
	/// Anything else; the tokens are kept.
	Unavailable(Error),
}

/// OpenAI's OAuth and model-list endpoints. Implementations never log, event
/// or copy token responses into errors.
#[async_trait]
pub trait Authorization: Send + Sync {
	async fn exchange(&self, request: ExchangeRequest<'_>) -> Result<Issued>;
	/// Verifies the signature, issuer, audience (the issued client), expiry
	/// and `nonce` of the ID token from an exchange, and returns its verified
	/// `sub`: the ChatGPT account the grant was issued for.
	async fn verify_id_token(
		&self,
		id_token: &SecretString,
		client_id: &str,
		nonce: &str,
	) -> Result<String>;
	async fn refresh(
		&self,
		client_id: &str,
		refresh_token: &SecretString,
	) -> std::result::Result<Issued, RefreshFailure>;
	/// RFC 7009 revocation. An already-invalid token also succeeds.
	async fn revoke(&self, client_id: &str, refresh_token: &SecretString) -> Result<()>;
	/// The owner's model list, `visibility == "list"` entries only.
	async fn models(&self, access_token: &SecretString) -> Result<Vec<PlanModel>>;
}

/// One owner's scope. Every write of the owner's Plan Connections, including
/// a call's refresh, holds it, so a rotating refresh token is never raced.
#[async_trait]
pub trait Scope: Send {
	async fn owned(&mut self, provider: Provider) -> Result<Option<PlanConnection>>;
	/// External Identities that are not disabled and hold an enabled Mapping to
	/// the owner. Identity Disablement therefore pauses a connection (zero)
	/// without deleting it.
	async fn mapped_identities(&mut self) -> Result<usize>;
	async fn insert(&mut self, value: &PlanConnection, event: &str, actor: &str) -> Result<()>;
	async fn save(&mut self, value: &PlanConnection, event: &str, actor: &str) -> Result<()>;
	async fn delete(&mut self, value: &PlanConnection, event: &str, actor: &str) -> Result<()>;
	async fn commit(self: Box<Self>) -> Result<()>;
}
#[async_trait]
pub trait Repository: Send + Sync {
	async fn begin(&self, owner: &Owner) -> Result<Box<dyn Scope>>;
	/// The owner of one connection, read without the lock to pick a scope.
	async fn owner_of(&self, id: Uuid) -> Result<Option<Owner>>;
	/// The owners of every Plan Connection in one Tenant, read without locks
	/// to pick scopes.
	async fn owners(&self, tenant: &str) -> Result<Vec<Owner>>;
}

/// What a pending authorization contributes to its completion.
pub struct Pending<'a> {
	/// The `state` generated for this attempt; the callback must return it.
	pub state: &'a str,
	pub verifier: &'a SecretString,
	pub nonce: &'a str,
	pub redirect_uri: &'a str,
	pub saved_client: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "PlanAuthorizationOutcome")]
pub enum Outcome {
	Connected,
	Reconnected,
	Denied,
	Failed,
	/// The grant lacked `chatgpt.tokens.use.direct`. Its tokens were revoked;
	/// only the issued client and account are kept, so enabling plan usage
	/// later reauthorizes the same client (OpenAI's recovery guidance).
	PlanUsageNotGranted,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[schemars(rename = "PlanConnectionMetadata")]
pub struct Metadata {
	pub id: Uuid,
	pub tenant: String,
	pub subject: String,
	pub provider: Provider,
	pub state: State,
	pub connected_at: DateTime<Utc>,
	pub refreshed_at: Option<DateTime<Utc>>,
	/// When the connection needs reconnection unless a call refreshes it.
	pub refresh_expires_at: DateTime<Utc>,
	pub revision: i64,
}
impl From<&PlanConnection> for Metadata {
	fn from(value: &PlanConnection) -> Self {
		Self {
			id: value.id,
			tenant: value.tenant.clone(),
			subject: value.subject.clone(),
			provider: value.provider,
			state: value.state,
			connected_at: value.connected_at,
			refreshed_at: value.refreshed_at,
			refresh_expires_at: value.refresh_expires_at,
			revision: value.revision,
		}
	}
}

/// The result of a disconnect. OpenAI may be unreachable; Aidash's copy is
/// deleted either way, and the owner is told when revocation is unconfirmed.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[schemars(rename = "PlanDisconnection")]
pub struct Disconnection {
	pub revoked: bool,
}

/// Plan Tokens as one Store version.
#[derive(Serialize, Deserialize)]
struct Stored {
	access_token: String,
	refresh_token: String,
}
struct Tokens {
	access: SecretString,
	refresh: SecretString,
}
impl Tokens {
	fn seal(issued: &Issued) -> SecretString {
		serde_json::to_string(&Stored {
			access_token: issued.access_token.expose_secret().to_owned(),
			refresh_token: issued.refresh_token.expose_secret().to_owned(),
		})
		.expect("token JSON serializes")
		.into()
	}
	fn open(value: &SecretString) -> Result<Self> {
		let stored: Stored = serde_json::from_str(value.expose_secret())
			.map_err(|_| Error::Invalid("unreadable Plan Tokens".into()))?;
		Ok(Self {
			access: stored.access_token.into(),
			refresh: stored.refresh_token.into(),
		})
	}
}

fn issued_lifetime(issued: &Issued) -> Result<Duration> {
	if !(1..=86_400).contains(&issued.expires_in)
		|| issued.access_token.expose_secret().is_empty()
		|| issued.refresh_token.expose_secret().is_empty()
	{
		return Err(Error::External(
			"Sign in with ChatGPT returned unusable tokens".into(),
		));
	}
	Ok(Duration::seconds(issued.expires_in))
}

/// OpenAI's refresh gate for tokens that expire at `access_expires_at`. A gate
/// past that expiry would block every refresh, so it is clamped there.
fn earliest_refresh(issued: &Issued, access_expires_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
	issued
		.earliest_refresh_at
		.map(|at| at.min(access_expires_at))
}

/// A Store operation on something already gone counts as done, so a retried
/// cleanup completes.
fn gone(result: Result<()>) -> Result<()> {
	match result {
		Err(Error::NotFound(_)) => Ok(()),
		result => result,
	}
}

/// Retried with the usual bounded backoff, never a request to reconnect.
fn plan_unavailable() -> Error {
	Error::External("ChatGPT plan usage is temporarily unavailable".into())
}

pub struct Service {
	pub repository: Arc<dyn Repository>,
	pub store: Arc<dyn Store>,
	pub reader: Arc<dyn KeyMaterialReader>,
	pub authorization: Arc<dyn Authorization>,
}

impl Service {
	async fn tokens(&self, row: &PlanConnection, version: &str) -> Result<Tokens> {
		let secret = self
			.reader
			.read(&row.tenant, &row.secret_resource, version)
			.await?;
		Tokens::open(&secret)
	}

	/// The owner's connection, if any.
	pub async fn status(&self, owner: &Owner) -> Result<Option<Metadata>> {
		let mut scope = self.repository.begin(owner).await?;
		let row = scope.owned(Provider::Chatgpt).await?;
		scope.commit().await?;
		Ok(row.as_ref().map(Metadata::from))
	}

	/// Refuses before OpenAI issues anything when the owner may not hold a
	/// Plan Connection: a subject shared by several External Identities.
	pub async fn check(&self, owner: &Owner) -> Result<Option<String>> {
		let mut scope = self.repository.begin(owner).await?;
		let shared = scope.mapped_identities().await? != 1;
		let saved = scope
			.owned(Provider::Chatgpt)
			.await?
			.map(|row| row.client_id);
		scope.commit().await?;
		if shared {
			return Err(shared_subject());
		}
		Ok(saved)
	}

	/// Completes one authorization from the callback URL a desktop client
	/// relayed or the owner pasted. A declined or failed consent stores
	/// nothing and never reaches the exchange.
	pub async fn complete(
		&self,
		owner: &Owner,
		pending: Pending<'_>,
		callback: &Callback,
		actor: &str,
	) -> Result<Outcome> {
		// Bind the callback to the attempt that generated its state before
		// trusting anything else in it, including a denial.
		if callback.state != pending.state {
			return Err(Error::Invalid(
				"the callback does not belong to this Sign in with ChatGPT attempt".into(),
			));
		}
		let (code, returned_client) = match &callback.result {
			CallbackResult::Denied => return Ok(Outcome::Denied),
			CallbackResult::Failed => return Ok(Outcome::Failed),
			CallbackResult::Code { code, client_id } => (code, client_id.as_deref()),
		};
		let client = domain::issued_client(pending.saved_client, returned_client)?;
		// An attempt started with the connection's saved client renews that
		// connection; if it is gone by now, a disconnect ended it.
		let reconnecting = pending.saved_client.is_some();
		let issued = self
			.authorization
			.exchange(ExchangeRequest {
				client_id: &client,
				code,
				verifier: pending.verifier,
				redirect_uri: pending.redirect_uri,
			})
			.await?;
		let verified = match &issued.id_token {
			Some(id_token) => {
				self.authorization
					.verify_id_token(id_token, &client, pending.nonce)
					.await
			}
			None => Err(Error::Invalid(
				"Sign in with ChatGPT returned no ID token".into(),
			)),
		};
		// The ID token is only verified, never kept: Plan Tokens are the
		// access and refresh tokens alone. Its subject identifies the account.
		let account = match verified {
			Ok(account) => account,
			Err(error) => {
				self.revoke_quietly(&client, &issued.refresh_token).await;
				return Err(error);
			}
		};
		// A response may omit `scope` when it equals the requested one
		// (RFC 6749 §5.1), which includes plan usage; a reported scope must.
		if !issued
			.scope
			.as_deref()
			.is_none_or(domain::grants_plan_usage)
		{
			self.revoke_quietly(&client, &issued.refresh_token).await;
			// Keep the sign-in without plan usage: a later attempt reuses the
			// issued client instead of registering another one.
			if self
				.register(owner, &client, &account, reconnecting, actor)
				.await
				.is_err()
			{
				tracing::warn!("the issued client of a grant without plan usage was not kept");
			}
			return Ok(Outcome::PlanUsageNotGranted);
		}
		let lifetime = match issued_lifetime(&issued) {
			Ok(lifetime) => lifetime,
			Err(error) => {
				self.revoke_quietly(&client, &issued.refresh_token).await;
				return Err(error);
			}
		};
		match self
			.write(
				owner,
				client.clone(),
				account,
				&issued,
				lifetime,
				reconnecting,
				actor,
			)
			.await
		{
			Ok(outcome) => Ok(outcome),
			Err(error) => {
				// Tokens Aidash could not store must not stay live at OpenAI.
				self.revoke_quietly(&client, &issued.refresh_token).await;
				Err(error)
			}
		}
	}

	/// Records the issued client and the verified account before any Plan
	/// Token is stored. The row is the only pointer to its Store resource, so
	/// tokens are never written where a disconnect or reconnect cannot find
	/// them. Until tokens are pinned, the connection needs reconnection.
	/// A reconnection never creates one: the connection it renews was
	/// disconnected meanwhile, and recreating it would undo that.
	/// Returns whether this call created it.
	async fn register(
		&self,
		owner: &Owner,
		client: &str,
		account: &str,
		reconnecting: bool,
		actor: &str,
	) -> Result<bool> {
		let mut scope = self.repository.begin(owner).await?;
		if scope.mapped_identities().await? != 1 {
			return Err(shared_subject());
		}
		if scope.owned(Provider::Chatgpt).await?.is_some() {
			scope.commit().await?;
			return Ok(false);
		}
		if reconnecting {
			scope.commit().await?;
			return Err(Error::Conflict(
				"the Plan Connection was disconnected while reconnecting".into(),
			));
		}
		let now = Utc::now();
		let id = Uuid::now_v7();
		let resource = self.store.plan_resource(id);
		self.store.create_plan(&owner.tenant, id).await?;
		let row = PlanConnection {
			id,
			tenant: owner.tenant.clone(),
			subject: owner.subject.clone(),
			provider: Provider::Chatgpt,
			client_id: client.to_owned(),
			account: account.to_owned(),
			secret_resource: resource.clone(),
			pinned_version: None,
			state: State::NeedsReconnection,
			access_expires_at: now,
			earliest_refresh_at: None,
			refresh_expires_at: now,
			connected_at: now,
			refreshed_at: None,
			revision: 1,
		};
		let inserted = match scope
			.insert(&row, "plan_connection.registered", actor)
			.await
		{
			Ok(()) => scope.commit().await,
			Err(error) => Err(error),
		};
		if let Err(error) = inserted {
			// The resource holds no tokens yet.
			self.delete_resource(&resource).await;
			return Err(error);
		}
		Ok(true)
	}

	#[allow(clippy::too_many_arguments)]
	async fn write(
		&self,
		owner: &Owner,
		client: String,
		account: String,
		issued: &Issued,
		lifetime: Duration,
		reconnecting: bool,
		actor: &str,
	) -> Result<Outcome> {
		let first = self
			.register(owner, &client, &account, reconnecting, actor)
			.await?;
		let mut scope = self.repository.begin(owner).await?;
		if scope.mapped_identities().await? != 1 {
			return Err(shared_subject());
		}
		let mut row = scope.owned(Provider::Chatgpt).await?.ok_or_else(|| {
			Error::Conflict("the Plan Connection was disconnected while connecting".into())
		})?;
		// A reconnection keeps the connection and its issued client, and must
		// authenticate the same ChatGPT account it was issued for.
		if row.client_id != client {
			return Err(Error::Invalid(
				"Sign in with ChatGPT returned a different client".into(),
			));
		}
		if row.account != account {
			return Err(Error::Conflict(
				"Sign in with ChatGPT authenticated a different ChatGPT account; \
				 disconnect this Plan Connection before connecting another account"
					.into(),
			));
		}
		let now = Utc::now();
		let stale = self.stale_versions(&row).await;
		let version = self
			.store
			.add_version(&owner.tenant, &row.secret_resource, &Tokens::seal(issued))
			.await?;
		row.pinned_version = Some(version.clone());
		row.state = State::Active;
		row.access_expires_at = now + lifetime;
		row.earliest_refresh_at = earliest_refresh(issued, row.access_expires_at);
		row.refresh_expires_at = now + domain::refresh_lifetime();
		row.connected_at = now;
		row.refreshed_at = None;
		row.revision += 1;
		let event = if first {
			"plan_connection.connected"
		} else {
			"plan_connection.reconnected"
		};
		let saved = match scope.save(&row, event, actor).await {
			Ok(()) => scope.commit().await,
			Err(error) => Err(error),
		};
		if let Err(error) = saved {
			self.destroy_quietly(&version).await;
			return Err(error);
		}
		for version in stale {
			self.destroy_quietly(&version).await;
		}
		Ok(if first {
			Outcome::Connected
		} else {
			Outcome::Reconnected
		})
	}

	/// The versions to destroy once a new one is pinned: everything the
	/// resource holds now, including versions an earlier cleanup failed to
	/// destroy. Read under the owner's scope, before the new version is added.
	async fn stale_versions(&self, row: &PlanConnection) -> Vec<String> {
		self.store
			.versions(&row.secret_resource)
			.await
			.unwrap_or_else(|_| row.pinned_version.iter().cloned().collect())
	}

	/// Revokes the grant at OpenAI, then deletes Aidash's copy. A revocation
	/// OpenAI did not confirm still deletes the copy and reports it.
	pub async fn disconnect(
		&self,
		owner: &Owner,
		expected_revision: i64,
		actor: &str,
	) -> Result<Disconnection> {
		let mut scope = self.repository.begin(owner).await?;
		let row = scope
			.owned(Provider::Chatgpt)
			.await?
			.ok_or_else(|| Error::NotFound("Plan Connection not found".into()))?;
		row.check_revision(expected_revision)?;
		let revoked = self.revoke_row(&row).await;
		// The row is the only pointer to the tokens: delete them first, so a
		// failed cleanup leaves the connection for a retried disconnect.
		self.delete_tokens(&row).await?;
		scope
			.delete(&row, "plan_connection.disconnected", actor)
			.await?;
		scope.commit().await?;
		Ok(Disconnection { revoked })
	}

	/// Ends every Plan Connection of an owner who lost access: a removed or
	/// disabled Mapping, or leaving the Tenant (ADR 0007).
	pub async fn retire(&self, owner: &Owner, actor: &str) -> Result<()> {
		let mut scope = self.repository.begin(owner).await?;
		let Some(row) = scope.owned(Provider::Chatgpt).await? else {
			return scope.commit().await;
		};
		self.revoke_row(&row).await;
		self.delete_tokens(&row).await?;
		scope.delete(&row, "plan_connection.retired", actor).await?;
		scope.commit().await
	}

	/// Ends every Plan Connection in one Tenant, as recording a Tenant
	/// Deletion requires before any purge (#239).
	pub async fn retire_tenant(&self, tenant: &str, actor: &str) -> Result<()> {
		for owner in self.repository.owners(tenant).await? {
			self.retire(&owner, actor).await?;
		}
		Ok(())
	}

	/// Whether the connection admission pinned may serve a call now. Unlike
	/// [`Self::bearer`] it neither reads Plan Tokens nor refreshes them.
	pub async fn usable(&self, id: Uuid) -> Result<()> {
		let owner = self
			.repository
			.owner_of(id)
			.await?
			.ok_or(Error::Plan(Failure::NotConnected))?;
		let mut scope = self.repository.begin(&owner).await?;
		let row = scope
			.owned(Provider::Chatgpt)
			.await?
			.filter(|row| row.id == id)
			.ok_or(Error::Plan(Failure::NotConnected))?;
		let mapped = scope.mapped_identities().await?;
		scope.commit().await?;
		if mapped != 1 {
			return Err(Error::Plan(Failure::NotConnected));
		}
		row.require_usable(Utc::now())
			.map(drop)
			.map_err(Error::Plan)
	}

	/// Records that a connection's pinned Plan Tokens became unreadable with a
	/// lost Master Key. They cannot be revoked at OpenAI any more, so the
	/// owner must connect again; the Store removes the versions itself. The
	/// pin is kept although nothing can read it: a later disconnection then
	/// fails to read the refresh token and reports the grant as not revoked,
	/// where no pin would claim it had nothing left to revoke.
	pub async fn mark_unreadable(&self, id: Uuid, actor: &str) -> Result<()> {
		let Some(owner) = self.repository.owner_of(id).await? else {
			return Ok(());
		};
		let mut scope = self.repository.begin(&owner).await?;
		let Some(mut row) = scope.owned(Provider::Chatgpt).await?.filter(|row| {
			row.id == id && row.pinned_version.is_some() && row.state != State::NeedsReconnection
		}) else {
			return scope.commit().await;
		};
		row.state = State::NeedsReconnection;
		row.revision += 1;
		scope
			.save(&row, "plan_connection.needs_reconnection", actor)
			.await?;
		scope.commit().await
	}

	/// The access token for one call on the connection admission pinned.
	/// Refreshes only when the token is about to expire, serialized under the
	/// owner's scope.
	pub async fn bearer(&self, id: Uuid) -> Result<SecretString> {
		self.access(id, false).await
	}

	/// Forces one refresh after OpenAI rejected the access token. Returns
	/// whether the caller may retry with the new token.
	pub async fn refresh_rejected(&self, id: Uuid) -> Result<bool> {
		self.access(id, true).await.map(|_| true)
	}

	async fn access(&self, id: Uuid, force: bool) -> Result<SecretString> {
		let owner = self
			.repository
			.owner_of(id)
			.await?
			.ok_or(Error::Plan(Failure::NotConnected))?;
		let mut scope = self.repository.begin(&owner).await?;
		let row = scope
			.owned(Provider::Chatgpt)
			.await?
			.filter(|row| row.id == id)
			.ok_or(Error::Plan(Failure::NotConnected))?;
		if scope.mapped_identities().await? != 1 {
			return Err(Error::Plan(Failure::NotConnected));
		}
		let now = Utc::now();
		let version = match row.require_usable(now) {
			Ok(version) => version.to_owned(),
			// Unused past its refresh lifetime: record that it needs
			// reconnection and destroy its tokens, not only refuse the call.
			Err(Failure::NeedsReconnection) if row.state == State::Active => {
				self.discard(scope, row, State::NeedsReconnection).await?;
				return Err(Error::Plan(Failure::NeedsReconnection));
			}
			Err(failure) => return Err(Error::Plan(failure)),
		};
		// OpenAI may refuse a refresh before its `earliest_refresh_at`, and a
		// refused refresh would end the grant: wait instead.
		if force && !row.may_refresh(now) {
			scope.commit().await?;
			return Err(plan_unavailable());
		}
		let tokens = self.tokens(&row, &version).await?;
		if !force && !row.needs_refresh(now) {
			scope.commit().await?;
			return Ok(tokens.access);
		}
		self.refresh(scope, row, tokens, now).await
	}

	async fn refresh(
		&self,
		mut scope: Box<dyn Scope>,
		mut row: PlanConnection,
		tokens: Tokens,
		now: DateTime<Utc>,
	) -> Result<SecretString> {
		let issued = match self
			.authorization
			.refresh(&row.client_id, &tokens.refresh)
			.await
		{
			Ok(issued) => issued,
			Err(RefreshFailure::Terminal) => {
				self.discard(scope, row, State::NeedsReconnection).await?;
				return Err(Error::Plan(Failure::NeedsReconnection));
			}
			Err(RefreshFailure::Unavailable(error)) => {
				scope.commit().await?;
				return Err(error);
			}
		};
		// A refresh omits `scope` to keep the grant; a reported scope set
		// without plan usage means the grant no longer authorizes it.
		if issued
			.scope
			.as_deref()
			.is_some_and(|scope| !domain::grants_plan_usage(scope))
		{
			self.revoke_quietly(&row.client_id, &issued.refresh_token)
				.await;
			self.discard(scope, row, State::NeedsReconnection).await?;
			return Err(Error::Plan(Failure::NeedsReconnection));
		}
		let lifetime = match issued_lifetime(&issued) {
			Ok(lifetime) => lifetime,
			Err(error) => {
				drop(scope);
				return Err(self.abandon_rotation(&row, &issued, error).await);
			}
		};
		let stale = self.stale_versions(&row).await;
		// Store the rotated pair before anything else: the old refresh token
		// is already spent.
		let version = match self
			.store
			.add_version(&row.tenant, &row.secret_resource, &Tokens::seal(&issued))
			.await
		{
			Ok(version) => version,
			Err(error) => {
				drop(scope);
				return Err(self.abandon_rotation(&row, &issued, error).await);
			}
		};
		let spent = row.clone();
		row.pinned_version = Some(version);
		row.access_expires_at = now + lifetime;
		row.earliest_refresh_at = earliest_refresh(&issued, row.access_expires_at);
		row.refresh_expires_at = now + domain::refresh_lifetime();
		row.refreshed_at = Some(now);
		row.revision += 1;
		if let Err(error) = scope
			.save(&row, "plan_connection.refreshed", SYSTEM_ACTOR)
			.await
		{
			drop(scope);
			return Err(self.abandon_rotation(&spent, &issued, error).await);
		}
		if let Err(error) = scope.commit().await {
			return Err(self.abandon_rotation(&spent, &issued, error).await);
		}
		for version in stale {
			self.destroy_quietly(&version).await;
		}
		Ok(issued.access_token)
	}

	/// A refresh whose rotated pair could not be kept. The old refresh token
	/// is spent, so the connection cannot be used again: the new grant is
	/// revoked rather than left live at OpenAI, and the connection asks for
	/// reconnection. Every stored version, including any written for the new
	/// pair, is destroyed. If that transition cannot be written either, the
	/// next call's refresh with the spent token makes it.
	async fn abandon_rotation(
		&self,
		spent: &PlanConnection,
		issued: &Issued,
		error: Error,
	) -> Error {
		self.revoke_quietly(&spent.client_id, &issued.refresh_token)
			.await;
		let owner = Owner {
			tenant: spent.tenant.clone(),
			subject: spent.subject.clone(),
		};
		let recovered = async {
			let mut scope = self.repository.begin(&owner).await?;
			// Only the state whose token was spent: a reconnect that completed
			// meanwhile has a newer revision and keeps its fresh grant.
			match scope.owned(Provider::Chatgpt).await?.filter(|row| {
				row.id == spent.id
					&& row.revision == spent.revision
					&& row.pinned_version == spent.pinned_version
			}) {
				Some(row) => self.discard(scope, row, State::NeedsReconnection).await,
				None => scope.commit().await,
			}
		}
		.await;
		match recovered {
			Ok(()) => Error::Plan(Failure::NeedsReconnection),
			Err(_) => {
				tracing::warn!(
					"a Plan Connection whose refresh could not be stored was not marked for reconnection"
				);
				error
			}
		}
	}

	/// Marks the connection [`State::Ineligible`] after OpenAI reported that
	/// the owner's plan cannot be used; its owner is notified by the event.
	pub async fn mark_ineligible(&self, id: Uuid) -> Result<()> {
		let Some(owner) = self.repository.owner_of(id).await? else {
			return Ok(());
		};
		let mut scope = self.repository.begin(&owner).await?;
		let Some(mut row) = scope
			.owned(Provider::Chatgpt)
			.await?
			.filter(|row| row.id == id && row.state == State::Active)
		else {
			return scope.commit().await;
		};
		row.state = State::Ineligible;
		row.revision += 1;
		scope
			.save(&row, "plan_connection.ineligible", SYSTEM_ACTOR)
			.await?;
		scope.commit().await
	}

	/// The owner's ChatGPT model list, read with their own Plan Connection.
	pub async fn models(&self, owner: &Owner) -> Result<Vec<PlanModel>> {
		let mut scope = self.repository.begin(owner).await?;
		let id = scope
			.owned(Provider::Chatgpt)
			.await?
			.map(|row| row.id)
			.ok_or(Error::Plan(Failure::NotConnected))?;
		scope.commit().await?;
		let access = self.bearer(id).await?;
		let result = match self.authorization.models(&access).await {
			// OpenAI can reject an access token before its recorded expiry.
			// As for inference, one forced refresh decides whether the owner
			// must reconnect.
			Err(Error::Plan(Failure::NeedsReconnection)) => {
				let access = self.access(id, true).await?;
				match self.authorization.models(&access).await {
					// OpenAI asks for a new sign-in only after a terminal refresh
					// error or a confirmed revocation; a fresh token's rejection
					// is neither.
					Err(Error::Plan(Failure::NeedsReconnection)) => Err(plan_unavailable()),
					result => result,
				}
			}
			result => result,
		};
		if matches!(result, Err(Error::Plan(Failure::Ineligible))) {
			self.mark_ineligible(id).await?;
		}
		result
	}

	/// Ends use after a rejected refresh: the tokens are destroyed, while the
	/// row keeps the issued client for reconnection.
	async fn discard(
		&self,
		mut scope: Box<dyn Scope>,
		mut row: PlanConnection,
		state: State,
	) -> Result<()> {
		let event = match state {
			State::Ineligible => "plan_connection.ineligible",
			State::Active | State::NeedsReconnection => "plan_connection.needs_reconnection",
		};
		row.state = state;
		let pinned = row.pinned_version.take();
		row.revision += 1;
		// Destroy the tokens while the owner's scope still serializes writers:
		// a reconnect waiting on it pins a new version in the same resource,
		// which must survive this cleanup. A failed cleanup commits nothing,
		// so the call that next finds the connection retries the transition;
		// the pinned version goes last, so until then it stays readable.
		let mut versions = self.store.versions(&row.secret_resource).await?;
		versions.sort_by_key(|version| Some(version) == pinned.as_ref());
		for version in versions {
			gone(self.store.destroy(&version).await)?;
		}
		scope.save(&row, event, SYSTEM_ACTOR).await?;
		scope.commit().await
	}

	async fn revoke_row(&self, row: &PlanConnection) -> bool {
		let Some(version) = row.pinned_version.as_deref() else {
			return true;
		};
		match self.tokens(row, version).await {
			Ok(tokens) => self
				.authorization
				.revoke(&row.client_id, &tokens.refresh)
				.await
				.is_ok(),
			Err(_) => false,
		}
	}

	async fn revoke_quietly(&self, client: &str, refresh: &SecretString) {
		if self.authorization.revoke(client, refresh).await.is_err() {
			tracing::warn!("Plan Connection revocation was not confirmed");
		}
	}

	/// Deletes every Plan Token version and the resource. A resource that is
	/// already gone counts as deleted, so a retried cleanup completes.
	async fn delete_tokens(&self, row: &PlanConnection) -> Result<()> {
		let versions = match self.store.versions(&row.secret_resource).await {
			Err(Error::NotFound(_)) => Vec::new(),
			versions => versions?,
		};
		for version in versions {
			gone(self.store.destroy(&version).await)?;
		}
		gone(self.store.delete(&row.secret_resource).await)
	}

	async fn destroy_quietly(&self, version: &str) {
		if self.store.destroy(version).await.is_err() {
			tracing::warn!("Plan Token version cleanup failed");
		}
	}

	async fn delete_resource(&self, resource: &str) {
		if self.store.delete(resource).await.is_err() {
			tracing::warn!("Plan Token resource cleanup failed");
		}
	}
}

fn shared_subject() -> Error {
	Error::Conflict(
		"a subject that more than one External Identity is mapped to cannot hold a Plan Connection"
			.into(),
	)
}

#[cfg(test)]
mod tests;
