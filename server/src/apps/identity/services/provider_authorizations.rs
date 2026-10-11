//! Server-run Provider Authorization (OpenRouter OAuth PKCE). The browser only
//! carries the consent redirect; the host keeps `state` and the verifier,
//! exchanges the code and writes Key Material straight into the Store.
use super::oidc::{self, BrowserOrigin};
use super::provider_credentials::Management;
use crate::apps::identity::models::ProviderAuthorization;
use crate::apps::identity::serializers::provider_authorizations::{
	CallbackQuery, Outcome, Start, Started,
};
use crate::{Error, Result, authorization::identity::Actor, federation::Federation};
use aidash_application::provider_authorization::{self as authorization, Target};
use aidash_domain::provider_credentials::Provider;
use chrono::{Duration, Utc};
use http::{HeaderMap, Method, StatusCode};
use reinhardt::{Response, injectable};
use secrecy::{ExposeSecret, SecretString};

/// Binds pending authorizations to the browser that started them; the
/// dashboard session must also match at the callback.
pub(crate) const COOKIE: &str = "__Host-aidash-provider-authorization";
const DEFAULT_RETURN: &str = "/settings?view=providerCredentials";

#[derive(Clone)]
pub struct ProviderAuthorizations {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> ProviderAuthorizations {
	ProviderAuthorizations { runtime }
}

fn hex_digest(value: &str) -> String {
	oidc::digest(value)
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}
fn callback_uri(public_origin: &str, provider: Provider) -> String {
	format!(
		"{public_origin}/auth/provider-authorizations/{}/callback",
		provider.id()
	)
}
fn with_outcome(return_to: &str, outcome: Outcome) -> String {
	let separator = if return_to.contains('?') { '&' } else { '?' };
	format!(
		"{return_to}{separator}provider_authorization={}",
		outcome.as_str()
	)
}

impl ProviderAuthorizations {
	fn management(&self) -> Management {
		Management {
			runtime: self.runtime.clone(),
		}
	}
	/// Starts one authorization for a browser dashboard session. Bearer-token
	/// callers have no browser to bind and are refused.
	pub(crate) async fn start(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		headers: HeaderMap,
		tenant: String,
		provider: String,
		input: Start,
	) -> Result<Response> {
		let f = &self.runtime;
		let config = oidc::required_config(f)?;
		let provider = Provider::parse(&provider)?;
		let origin = origin.ok_or(Error::Forbidden)?;
		let session = oidc::browser_session_from_headers(f, &headers).await?;
		if session.id != origin.session.id {
			return Err(Error::Forbidden);
		}
		let context = headers
			.get("x-aidash-context")
			.and_then(|value| value.to_str().ok())
			.ok_or(Error::Forbidden)?
			.to_owned();
		let return_to =
			oidc::return_path(input.return_to.as_deref().or(Some(DEFAULT_RETURN)))?.to_owned();
		let target = match (input.provider_credential_id, input.expected_revision) {
			(None, None) => Target::Connect {
				id: uuid::Uuid::now_v7(),
			},
			(Some(id), Some(expected_revision)) => Target::Reconnect {
				id,
				expected_revision,
			},
			_ => {
				return Err(Error::Invalid(
					"reconnect requires provider_credential_id and expected_revision".into(),
				));
			}
		};
		self.management()
			.permit_authorization(&actor, &tenant, provider, target)
			.await?;
		let callback = callback_uri(config.public_origin, provider);
		let request = authorization::begin(provider, &callback, &tenant)?;
		let browser = oidc::cookie_value(&headers, &oidc::cookie_name(COOKIE, config))
			.map(str::to_owned)
			.unwrap_or_else(oidc::random_secret);
		let (credential, expected_revision) = match target {
			Target::Connect { id } => (id, None),
			Target::Reconnect {
				id,
				expected_revision,
			} => (id, Some(expected_revision)),
		};
		let pending = ProviderAuthorization::build()
			.state_hash(hex_digest(request.state.expose_secret()))
			.browser_hash(hex_digest(&browser))
			.session_id(session.id)
			.context(context)
			.tenant(tenant)
			.provider(provider.id().to_owned())
			.provider_credential_id(credential)
			.expected_revision(expected_revision)
			.pkce_verifier(request.verifier.expose_secret().to_owned())
			.callback_uri(callback)
			.return_to(return_to)
			.expires_at(Utc::now() + Duration::minutes(authorization::LIFETIME_MINUTES))
			.finish();
		let lease = f.store.orm_connection()?;
		pending.reserve(lease.handle()).await?;
		let mut response = Response::ok().with_json(&Started {
			authorization_url: request.url,
		})?;
		oidc::set_cookie(
			&mut response,
			COOKIE,
			&browser,
			config,
			true,
			authorization::LIFETIME_MINUTES * 60,
		)?;
		oidc::no_store(&mut response);
		Ok(response)
	}

	/// Finishes one authorization. A missing, replayed, expired or
	/// cross-session `state` fails before any provider call; a rejected code or
	/// verifier fails at the exchange. Neither stores anything.
	pub(crate) async fn callback(
		&self,
		headers: HeaderMap,
		provider: String,
		query: CallbackQuery,
	) -> Result<Response> {
		let f = &self.runtime;
		let config = oidc::required_config(f)?;
		let provider = Provider::parse(&provider)?;
		let browser = oidc::cookie_value(&headers, &oidc::cookie_name(COOKIE, config))
			.ok_or(Error::Unauthorized)?;
		let state = query.state.as_deref().ok_or(Error::Unauthorized)?;
		let lease = f.store.orm_connection()?;
		let pending = ProviderAuthorization::consume(lease.handle(), hex_digest(state)).await?;
		if pending.browser_hash != hex_digest(browser)
			|| pending.provider != provider.id()
			|| pending.callback_uri != callback_uri(config.public_origin, provider)
		{
			return Err(Error::Unauthorized);
		}
		let session = oidc::browser_session_from_headers(f, &headers).await?;
		if session.id != pending.session_id {
			return Err(Error::Unauthorized);
		}
		// Re-derive the actor under the original context with live account,
		// mapping and operator-grant checks.
		let mut scoped = headers.clone();
		scoped.insert(
			"x-aidash-context",
			pending.context.parse().map_err(|_| Error::Unauthorized)?,
		);
		let (actor, origin) = oidc::actor_from_headers(f, &scoped, &Method::GET).await?;
		if origin.session.id != pending.session_id {
			return Err(Error::Unauthorized);
		}
		let target = match pending.expected_revision {
			None => Target::Connect {
				id: pending.provider_credential_id,
			},
			Some(expected_revision) => Target::Reconnect {
				id: pending.provider_credential_id,
				expected_revision,
			},
		};
		let outcome = match query.code.as_deref() {
			// The user declined, or the provider reported an error.
			None => Outcome::Denied,
			Some(code) => {
				let verifier = SecretString::from(pending.pkce_verifier);
				match self
					.management()
					.complete_authorization(
						actor.clone(),
						pending.tenant.clone(),
						provider,
						target,
						code,
						&verifier,
					)
					.await
				{
					Ok(_) => {
						if let Target::Connect { id } = target {
							self.management()
								.bind_if_unbound(actor, pending.tenant.clone(), provider, id)
								.await;
							Outcome::Connected
						} else {
							Outcome::Reconnected
						}
					}
					Err(error) => {
						// Static fields only: errors never carry Key Material, but the
						// provider's code is still a bearer for ten minutes.
						let reason = match error {
							Error::Invalid(_) => "rejected",
							Error::Conflict(_) => "conflict",
							Error::Forbidden | Error::Unauthorized => "forbidden",
							_ => "unavailable",
						};
						tracing::warn!(
							provider = provider.id(),
							reason,
							"Provider Authorization failed"
						);
						Outcome::Failed
					}
				}
			}
		};
		let mut response = Response::new(StatusCode::SEE_OTHER)
			.with_location(&with_outcome(&pending.return_to, outcome));
		oidc::no_store(&mut response);
		Ok(response)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn outcomes_append_to_the_validated_return_path() {
		assert_eq!(
			with_outcome("/settings?view=providerCredentials", Outcome::Connected),
			"/settings?view=providerCredentials&provider_authorization=connected"
		);
		assert_eq!(
			with_outcome("/settings", Outcome::Failed),
			"/settings?provider_authorization=failed"
		);
		assert_eq!(
			callback_uri("https://aidash.example", Provider::Openrouter),
			"https://aidash.example/auth/provider-authorizations/openrouter/callback"
		);
	}
}
