//! Plan Connections (ADR 0007, ADR 0031): one subject's Sign in with ChatGPT
//! grant and the pure rules of OpenAI's plan-usage contract. Plan Tokens never
//! appear here; they stay in the Provider Credential Store.
use crate::provider_credentials::Provider;
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identity scopes for the ID token, plus the plan-usage grant.
pub const SCOPES: &str =
	"openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The scope without which a grant never authorizes plan usage. A valid ID
/// token alone proves nothing about it.
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// The resource indicator of every authorization, exchange and refresh.
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// The first-time registration entrypoint. It is never saved as a
/// connection's client ID.
pub const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
/// The name OpenAI shows for the registered client in ChatGPT settings.
pub const AGENT_NAME: &str = "Aidash";
/// OpenAI accepts only this loopback host; `localhost` is refused.
pub const LOOPBACK_HOST: &str = "127.0.0.1";
pub const CALLBACK_PATH: &str = "/auth/callback";
/// The loopback port of a pasted-URL authorization. Nothing listens there, so
/// the browser shows an error page whose address the user copies into Aidash.
/// It differs from Codex CLI's 1455, whose listener would answer instead.
pub const PASTE_PORT: u16 = 1457;
/// Where owners review their plan and this app's limit.
pub const USAGE_SETTINGS_URL: &str = "https://chatgpt.com/settings/usage";
/// A call refreshes an access token this close to its expiry.
const REFRESH_MARGIN_SECONDS: i64 = 120;
const MAX_CALLBACK_BYTES: usize = 8192;

/// OpenAI's refresh token lifetime. Each refresh starts a new one, so a
/// connection unused for this long needs reconnection.
pub fn refresh_lifetime() -> Duration {
	Duration::days(30)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "PlanConnectionState")]
pub enum State {
	/// Usable by Runs whose delegation chain starts at the owner.
	Active,
	/// A refresh was rejected, or OpenAI confirmed that the grant ended. The
	/// owner must connect again; the Plan Tokens are already discarded.
	NeedsReconnection,
	/// OpenAI reports that the owner's plan or workspace is not eligible.
	/// Reconnecting does not help, so the owner is not asked to.
	Ineligible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanConnection {
	pub id: Uuid,
	pub tenant: String,
	/// The user subject that owns the connection. Nobody else's Runs use it.
	pub subject: String,
	/// The External Identity that connected it, re-recorded by a reconnection.
	/// Another identity mapped to the same subject never uses it.
	pub identity_id: Uuid,
	pub provider: Provider,
	/// OpenAI's issued client ID, bound to one ChatGPT account and workspace.
	/// It is not secret, but it is reused only for this connection.
	pub client_id: String,
	/// The verified ChatGPT account (the ID token's `sub`) the grant was
	/// issued for. A reconnection must authenticate the same account.
	pub account: String,
	/// Store references are adapter metadata, never included in API projections.
	pub secret_resource: String,
	pub pinned_version: Option<String>,
	pub state: State,
	pub access_expires_at: DateTime<Utc>,
	/// OpenAI's `earliest_refresh_at` from the last token response, never
	/// later than `access_expires_at`. No refresh is attempted before it.
	pub earliest_refresh_at: Option<DateTime<Utc>>,
	pub refresh_expires_at: DateTime<Utc>,
	pub connected_at: DateTime<Utc>,
	pub refreshed_at: Option<DateTime<Utc>>,
	pub revision: i64,
}
/// What a Run's admission pinned: one connection, as the identity it served
/// then. A pin never follows the connection to another identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
	pub connection: Uuid,
	pub identity: Uuid,
}
/// An External Identity that holds an enabled Mapping to a subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappedIdentity {
	pub id: Uuid,
	/// Identity Disablement keeps the Mapping; it only pauses what the
	/// identity holds.
	pub disabled: bool,
}

/// The identity that may connect or use a Plan Connection of the subject
/// these identities are mapped to: the only one that is not disabled. A
/// subject shared by several has none (ADR 0007).
pub fn sole_identity(mapped: &[MappedIdentity]) -> Option<Uuid> {
	let mut active = mapped.iter().filter(|identity| !identity.disabled);
	match (active.next(), active.next()) {
		(Some(identity), None) => Some(identity.id),
		_ => None,
	}
}

impl PlanConnection {
	/// Whether the identity that connected this is the only active one mapped
	/// to its subject. Restoring another identity mapped there while the owner
	/// is disabled never hands it the owner's plan.
	pub fn serves(&self, mapped: &[MappedIdentity]) -> bool {
		sole_identity(mapped) == Some(self.identity_id)
	}
	/// The pin an admission records for this connection now.
	pub fn pin(&self) -> Pin {
		Pin {
			connection: self.id,
			identity: self.identity_id,
		}
	}
	/// Whether `pin` still names this connection as the identity it serves.
	/// A pin read before a reconnection by another identity then fails,
	/// however late its call reaches the owner's lock.
	pub fn holds(&self, pin: Pin) -> bool {
		self.pin() == pin
	}
	/// Whether the identity that connected this still holds an enabled
	/// Mapping to its subject, even while disabled. Once it does not, the
	/// owner lost access and the connection is retired.
	pub fn owner_mapped(&self, mapped: &[MappedIdentity]) -> bool {
		mapped
			.iter()
			.any(|identity| identity.id == self.identity_id)
	}
	pub fn check_revision(&self, expected: i64) -> Result<()> {
		if self.revision != expected {
			return Err(Error::Conflict("stale Plan Connection revision".into()));
		}
		Ok(())
	}
	/// The pinned Store version of a connection that may serve a call now.
	pub fn require_usable(&self, now: DateTime<Utc>) -> std::result::Result<&str, Failure> {
		match self.state {
			State::Active if self.refresh_expires_at > now => self
				.pinned_version
				.as_deref()
				.ok_or(Failure::NeedsReconnection),
			State::Active | State::NeedsReconnection => Err(Failure::NeedsReconnection),
			State::Ineligible => Err(Failure::Ineligible),
		}
	}
	/// Refreshes happen only when a call needs a token (ADR 0007): never in
	/// the background, which would be maintenance touching the connection.
	/// An access token that still works is used until OpenAI permits a
	/// refresh; an expired one is refreshed regardless.
	pub fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
		self.access_expires_at <= now
			|| (self.access_expires_at - now <= Duration::seconds(REFRESH_MARGIN_SECONDS)
				&& self.may_refresh(now))
	}
	/// Whether OpenAI's `earliest_refresh_at` permits a refresh now.
	pub fn may_refresh(&self, now: DateTime<Utc>) -> bool {
		self.earliest_refresh_at.is_none_or(|at| at <= now)
	}
}

/// Why a Plan Connection cannot serve a call. Messages name the owner's next
/// step and never carry OpenAI's own text.
#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	thiserror::Error,
	Serialize,
	Deserialize,
	schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "PlanConnectionFailure")]
pub enum Failure {
	#[error(
		"the subject that started this Run has no usable ChatGPT Plan Connection; connect your ChatGPT plan in personal settings"
	)]
	NotConnected,
	#[error(
		"Plan Connections serve only Runs that their owner admits directly; this Run was admitted through Federation, by a Deployment Administrator or for maintenance"
	)]
	NotAdmitted,
	#[error(
		"the ChatGPT Plan Connection needs reconnection; reconnect your ChatGPT plan in personal settings"
	)]
	NeedsReconnection,
	#[error("the ChatGPT plan or workspace is not eligible for use in Aidash")]
	Ineligible,
	#[error(
		"usage limit reached: review your plan or this app's limit in ChatGPT settings (https://chatgpt.com/settings/usage)"
	)]
	UsageLimit,
	#[error("ChatGPT plan usage is restricted for this account, workspace or region")]
	Restricted,
	#[error("ChatGPT plan usage does not support part of this request: {0}")]
	Unsupported(Unsupported),
	#[error("ChatGPT plan usage rejected the request Aidash sent")]
	Misconfigured,
}

/// The part of a request that plan usage refused, from OpenAI's `error.param`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Unsupported {
	Model,
	Tools,
	Input,
	Reasoning,
	Other,
}
impl std::fmt::Display for Unsupported {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Self::Model => "model",
			Self::Tools => "tools",
			Self::Input => "input",
			Self::Reasoning => "reasoning",
			Self::Other => "request option",
		})
	}
}
impl Unsupported {
	/// Maps OpenAI's `error.param` to a fixed class, so no provider text leaks.
	pub fn from_param(param: Option<&str>) -> Self {
		let param = param.unwrap_or_default();
		let root = param.split(['.', '[']).next().unwrap_or_default();
		match root {
			"model" => Self::Model,
			"tools" | "tool_choice" => Self::Tools,
			"input" | "instructions" => Self::Input,
			"reasoning" => Self::Reasoning,
			_ => Self::Other,
		}
	}
}

/// How Aidash handles one rejected or failed plan call (ADR 0007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
	/// Fail the Run without retrying it.
	Fail(Failure),
	/// Mark the connection [`State::Ineligible`], notify its owner, fail the Run.
	MarkIneligible,
	/// Temporary unavailability: retry with the usual bounded backoff.
	Transient,
	/// The credential was not accepted: refresh once. Only a terminal refresh
	/// error marks the connection as needing reconnection.
	RefreshThenReconnect,
}

/// Classifies OpenAI's plan-usage failures by their exact `error.code`, then by
/// status. A 429 without the usage-limit code is an ordinary rate limit.
pub fn classify(status: u16, code: Option<&str>, param: Option<&str>) -> Disposition {
	match code {
		Some("subscription_sharing_usage_limit_exceeded") => Disposition::Fail(Failure::UsageLimit),
		Some(
			"subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable",
		) => Disposition::Transient,
		Some("subscription_sharing_user_not_eligible") => Disposition::MarkIneligible,
		Some("subscription_sharing_unsupported_capability") => {
			Disposition::Fail(Failure::Unsupported(Unsupported::from_param(param)))
		}
		Some("subscription_sharing_invalid_user") => Disposition::RefreshThenReconnect,
		Some("subscription_sharing_route_not_supported") => {
			Disposition::Fail(Failure::Misconfigured)
		}
		Some(code) if code.starts_with("chatpass_v2_") => Disposition::Fail(Failure::Misconfigured),
		_ => match status {
			401 => Disposition::RefreshThenReconnect,
			403 => Disposition::Fail(Failure::Restricted),
			408 | 429 | 500..=599 => Disposition::Transient,
			400 | 404 | 413 | 422 => {
				Disposition::Fail(Failure::Unsupported(Unsupported::from_param(param)))
			}
			_ => Disposition::Fail(Failure::Misconfigured),
		},
	}
}

/// Refresh errors that end the grant: the tokens are discarded and the owner
/// must connect again. Other refresh failures keep the tokens.
pub fn refresh_is_terminal(code: Option<&str>) -> bool {
	matches!(
		code,
		Some(
			"invalid_grant"
				| "invalid_refresh_token"
				| "token_expired"
				| "refresh_token_expired"
				| "refresh_token_invalidated"
				| "refresh_token_reused"
		)
	)
}

/// Granted scopes authorize plan usage only when they include [`PLAN_SCOPE`].
pub fn grants_plan_usage(scope: &str) -> bool {
	scope
		.split_whitespace()
		.any(|granted| granted == PLAN_SCOPE)
}

/// OpenAI's `ext_agent_host_id` for a Deployment: a stable `urn:uuid:` URN of
/// a random (version 4) UUID. It identifies the host, never authenticates it.
pub fn host_id(id: Uuid) -> Result<String> {
	if id.get_version_num() != 4 {
		return Err(Error::Invalid(
			"Plan Connection host ID must be a UUIDv4".into(),
		));
	}
	Ok(format!("urn:uuid:{id}"))
}

/// The loopback redirect URI of one authorization. Only its port may vary
/// between authorizations.
pub fn redirect_uri(port: u16) -> Result<String> {
	if port == 0 {
		return Err(Error::Invalid("loopback port must not be 0".into()));
	}
	Ok(format!("http://{LOOPBACK_HOST}:{port}{CALLBACK_PATH}"))
}

/// What a relayed or pasted callback URL carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callback {
	pub state: String,
	pub result: CallbackResult,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackResult {
	/// The user consented. `client_id` is present on a first registration and
	/// may be absent on a reauthorization.
	Code {
		code: String,
		client_id: Option<String>,
	},
	/// The user declined (`access_denied`).
	Denied,
	/// OpenAI reported another authorization error.
	Failed,
}

/// Parses the callback URL a desktop client relayed or a user pasted. It must
/// be the pending authorization's exact loopback redirect URI with a `state`;
/// any other URL is refused before its parameters are interpreted. The code it
/// carries is single-use and useless without the server-held PKCE verifier.
pub fn parse_callback(value: &str, redirect_uri: &str) -> Result<Callback> {
	let refused = || Error::Invalid("the URL is not this authorization's callback".into());
	let value = value.trim();
	if value.is_empty() || value.len() > MAX_CALLBACK_BYTES {
		return Err(refused());
	}
	let url = url::Url::parse(value).map_err(|_| refused())?;
	let expected = url::Url::parse(redirect_uri).map_err(|_| refused())?;
	if url.scheme() != "http"
		|| url.host_str() != Some(LOOPBACK_HOST)
		|| url.port().is_none()
		|| url.port() != expected.port()
		|| url.path() != CALLBACK_PATH
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.fragment().is_some()
	{
		return Err(refused());
	}
	let mut parameters = std::collections::BTreeMap::new();
	for (name, value) in url.query_pairs() {
		// A repeated parameter could be read differently by another parser.
		if parameters
			.insert(name.into_owned(), value.into_owned())
			.is_some()
		{
			return Err(refused());
		}
	}
	let bounded = |name: &str, max: usize| {
		parameters
			.get(name)
			.filter(|value| !value.is_empty() && value.len() <= max)
			.cloned()
	};
	let state = bounded("state", 512).ok_or_else(refused)?;
	if let Some(error) = parameters.get("error") {
		let result = if error == "access_denied" {
			CallbackResult::Denied
		} else {
			CallbackResult::Failed
		};
		return Ok(Callback { state, result });
	}
	let code = bounded("code", 4096).ok_or_else(refused)?;
	let client_id = match parameters.get("client_id") {
		None => None,
		Some(id)
			if !id.is_empty()
				&& id.len() <= 256
				&& id
					.bytes()
					.all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b)) =>
		{
			Some(id.clone())
		}
		Some(_) => return Err(refused()),
	};
	Ok(Callback {
		state,
		result: CallbackResult::Code { code, client_id },
	})
}

/// Which client ID an authorization or exchange uses, given the issued ID the
/// callback returned. A first registration must return one; a
/// reauthorization may omit it but must never return a different one.
pub fn issued_client(saved: Option<&str>, returned: Option<&str>) -> Result<String> {
	match (saved, returned) {
		(None, Some(issued)) if issued != DYNAMIC_CLIENT => Ok(issued.to_owned()),
		(None, _) => Err(Error::Invalid(
			"Sign in with ChatGPT did not complete client registration".into(),
		)),
		(Some(saved), None) => Ok(saved.to_owned()),
		(Some(saved), Some(returned)) if saved == returned => Ok(saved.to_owned()),
		(Some(_), Some(_)) => Err(Error::Invalid(
			"Sign in with ChatGPT returned a different client".into(),
		)),
	}
}

/// One entry of the owner's ChatGPT model list, offered to model pickers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "PlanModel")]
pub struct PlanModel {
	/// Sent as `model`.
	pub slug: String,
	pub display_name: String,
	/// Context window and output limit for the Registry form. Values OpenAI's
	/// list does not report come from [`DEFAULT_CONTEXT_WINDOW`] and
	/// [`DEFAULT_MAX_OUTPUT_TOKENS`], flagged by `defaulted`.
	pub context_window: usize,
	pub max_output_tokens: u32,
	pub defaulted: bool,
}
/// Conservative limits for a model whose list entry reports none. An
/// administrator reviews them in the Registry form before registering.
pub const DEFAULT_CONTEXT_WINDOW: usize = 128_000;
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 16_384;

#[cfg(test)]
mod tests;
