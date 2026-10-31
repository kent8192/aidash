//! Product-local browser sessions. OAuth credentials never enter browser JSON.
use crate::{Error, Marketplace, Result, json, store};
use chrono::{DateTime, Duration, Utc};
use http::StatusCode;
use openidconnect::{
	AuthType, AuthorizationCode, ClientId, ClientSecret, IssuerUrl, Nonce, OAuth2TokenResponse,
	PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, TokenResponse,
	core::{CoreClient, CoreProviderMetadata},
};
use reinhardt::http::{Request, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Browser sign-in follows the issuer's NIST SP 800-63B-4 AAL1 policy (#627):
/// 30 days overall and 7 days without use.
pub(crate) const SESSION_ABSOLUTE_SECONDS: i64 = 2_592_000;
pub(crate) const SESSION_IDLE_SECONDS: i64 = 604_800;

pub fn secret() -> String {
	format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
pub fn hash(value: &str) -> String {
	format!("{:x}", Sha256::digest(value.as_bytes()))
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
	pub account_id: Uuid,
	pub source_identity_id: Uuid,
	pub(crate) provider_subject: String,
	pub csrf_token: String,
	pub(crate) refresh_request_id: Uuid,
	pub(crate) access_token: String,
	pub(crate) refresh_token: Option<String>,
	pub(crate) created_at: DateTime<Utc>,
	pub(crate) last_used: DateTime<Utc>,
}
impl Session {
	/// Whether the local session has outlived its absolute or idle limit.
	pub(crate) fn expired(&self, now: DateTime<Utc>) -> bool {
		self.created_at + Duration::seconds(SESSION_ABSOLUTE_SECONDS) <= now
			|| self.last_used + Duration::seconds(SESSION_IDLE_SECONDS) <= now
	}
}
#[derive(Serialize, Deserialize)]
struct Login {
	verifier: String,
	nonce: String,
	created_at: DateTime<Utc>,
}
#[derive(Deserialize)]
pub struct Active {
	pub active: bool,
	pub sub: Option<String>,
	pub account_id: Option<Uuid>,
	pub source_identity_id: Option<Uuid>,
	pub client_id: Option<String>,
}
impl Marketplace {
	fn session_cookie(&self) -> &'static str {
		if self.config.secure() {
			"__Host-marketplace-session"
		} else {
			"marketplace-session"
		}
	}
	fn login_cookie(&self) -> &'static str {
		if self.config.secure() {
			"__Host-marketplace-login"
		} else {
			"marketplace-login"
		}
	}
	fn cookie(&self, name: &str, value: &str, max_age: i64) -> String {
		format!(
			"{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
			if self.config.secure() { "; Secure" } else { "" }
		)
	}
	pub(crate) fn cookie_value(&self, request: &Request, name: &str) -> Option<String> {
		request
			.headers
			.get("cookie")?
			.to_str()
			.ok()?
			.split(';')
			.filter_map(|v| v.trim().split_once('='))
			.find(|(k, _)| *k == name)
			.map(|(_, v)| v.to_owned())
	}
	pub(crate) fn csrf(&self, request: &Request, session: &Session) -> Result<()> {
		use subtle::ConstantTimeEq;
		let token = request.headers.get("x-marketplace-csrf");
		if request.headers.get("origin").and_then(|v| v.to_str().ok())
			!= Some(self.config.origin.trim_end_matches('/'))
			|| token.is_none_or(|v| !bool::from(v.as_bytes().ct_eq(session.csrf_token.as_bytes())))
		{
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	pub(crate) async fn authenticate(&self, request: &Request) -> Result<Session> {
		let token = self
			.cookie_value(request, self.session_cookie())
			.ok_or(Error::Unauthorized)?;
		let key = format!("session:{}", hash(&token));
		// Serializing the per-session refresh prevents two concurrent BFF requests
		// from turning a legitimate rotation into an inconsistent token replay.
		self.store
			.connection
			.atomic(async |tx| {
				store::lock_key(tx, &key).await?;
				let mut session: Session =
					store::get(tx, &key).await?.ok_or(Error::Unauthorized)?;
				let now = Utc::now();
				if session.expired(now) {
					return Err(Error::Unauthorized);
				}
				let mut active = self.introspect(&session.access_token).await?;
				if !active.active {
					let refresh = session.refresh_token.as_ref().ok_or(Error::Unauthorized)?;
					let response = self
						.http
						.post(format!(
							"{}/oauth/token",
							self.config.issuer.trim_end_matches('/')
						))
						.basic_auth(
							oauth_component(&self.config.client_id),
							Some(oauth_component(&self.config.client_secret)),
						)
						.header("Idempotency-Key", session.refresh_request_id.to_string())
						.form(&[
							("grant_type", "refresh_token"),
							("refresh_token", refresh.as_str()),
						])
						.send()
						.await
						.map_err(|_| Error::Unavailable)?;
					if !response.status().is_success() {
						return Err(Error::Unauthorized);
					}
					let result: serde_json::Value =
						response.json().await.map_err(|_| Error::Unavailable)?;
					session.access_token = result["access_token"]
						.as_str()
						.ok_or(Error::Unavailable)?
						.to_owned();
					session.refresh_token = Some(
						result["refresh_token"]
							.as_str()
							.ok_or(Error::Unavailable)?
							.to_owned(),
					);
					active = self.introspect(&session.access_token).await?;
					session.refresh_request_id = Uuid::new_v4();
				}
				if !active.active
					|| active.account_id != Some(session.account_id)
					|| active.sub.as_deref() != Some(session.provider_subject.as_str())
					|| active.source_identity_id != Some(session.source_identity_id)
					|| active.client_id.as_deref() != Some(self.config.client_id.as_str())
				{
					return Err(Error::Unauthorized);
				}
				session.last_used = now;
				store::put(tx, &key, "session", &session).await?;
				Ok(session)
			})
			.await
	}
	pub(crate) async fn introspect(&self, token: &str) -> Result<Active> {
		let response = self
			.http
			.post(format!(
				"{}/oauth/introspect",
				self.config.issuer.trim_end_matches('/')
			))
			.basic_auth(
				&self.config.resource_client_id,
				Some(&self.config.resource_secret),
			)
			.form(&[("token", token)])
			.send()
			.await
			.map_err(|_| Error::Unavailable)?;
		if !response.status().is_success() {
			return Err(Error::Unavailable);
		}
		response.json().await.map_err(|_| Error::Unavailable)
	}
	async fn metadata(&self) -> Result<(CoreProviderMetadata, openidconnect::reqwest::Client)> {
		let http = openidconnect::reqwest::Client::builder()
			.redirect(openidconnect::reqwest::redirect::Policy::none())
			.timeout(std::time::Duration::from_secs(10))
			.build()
			.map_err(|_| Error::Unavailable)?;
		let metadata = CoreProviderMetadata::discover_async(
			IssuerUrl::new(self.config.issuer.clone())
				.map_err(|_| Error::Invalid("invalid issuer"))?,
			&http,
		)
		.await
		.map_err(|_| Error::Unavailable)?;
		Ok((metadata, http))
	}
	pub(crate) async fn login(&self) -> Result<Response> {
		let (metadata, _) = self.metadata().await?;
		let client = CoreClient::from_provider_metadata(
			metadata,
			ClientId::new(self.config.client_id.clone()),
			Some(ClientSecret::new(self.config.client_secret.clone())),
		)
		.set_redirect_uri(
			RedirectUrl::new(self.config.callback())
				.map_err(|_| Error::Invalid("invalid callback"))?,
		);
		let state = secret();
		let nonce = secret();
		let verifier = secret();
		let challenge =
			PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(verifier.clone()));
		let csrf_state = state.clone();
		let auth_nonce = nonce.clone();
		let (url, _, _) = client
			.authorize_url(
				openidconnect::core::CoreAuthenticationFlow::AuthorizationCode,
				move || openidconnect::CsrfToken::new(csrf_state),
				move || Nonce::new(auth_nonce),
			)
			.set_pkce_challenge(challenge)
			.url();
		self.store
			.connection
			.atomic(async |tx| {
				store::put(
					tx,
					&format!("login:{}", hash(&state)),
					"login",
					&Login {
						verifier,
						nonce,
						created_at: Utc::now(),
					},
				)
				.await
			})
			.await?;
		Ok(Response::new(StatusCode::FOUND)
			.with_header("location", url.as_str())
			.with_header("set-cookie", &self.cookie(self.login_cookie(), &state, 600)))
	}
	pub(crate) async fn callback(&self, request: &Request) -> Result<Response> {
		let params: std::collections::HashMap<String, String> =
			url::form_urlencoded::parse(request.uri.query().unwrap_or("").as_bytes())
				.into_owned()
				.collect();
		let state = params.get("state").ok_or(Error::Unauthorized)?;
		if self.cookie_value(request, self.login_cookie()).as_deref() != Some(state.as_str()) {
			return Err(Error::Unauthorized);
		}
		let key = format!("login:{}", hash(state));
		let login: Login = self
			.store
			.connection
			.atomic(async |tx| {
				store::lock(tx).await?;
				let login = store::get(tx, &key).await?.ok_or(Error::Unauthorized)?;
				store::remove(tx, &key).await?;
				Ok::<_, Error>(login)
			})
			.await?;
		if login.created_at + Duration::minutes(10) <= Utc::now() {
			return Err(Error::Unauthorized);
		}
		let code = params.get("code").ok_or(Error::Unauthorized)?.clone();
		let (metadata, http) = self.metadata().await?;
		let client = CoreClient::from_provider_metadata(
			metadata,
			ClientId::new(self.config.client_id.clone()),
			Some(ClientSecret::new(self.config.client_secret.clone())),
		)
		.set_auth_type(AuthType::BasicAuth)
		.set_redirect_uri(
			RedirectUrl::new(self.config.callback()).map_err(|_| Error::Unauthorized)?,
		);
		let tokens = client
			.exchange_code(AuthorizationCode::new(code))
			.map_err(|_| Error::Unauthorized)?
			.set_pkce_verifier(PkceCodeVerifier::new(login.verifier))
			.request_async(&http)
			.await
			.map_err(|_| Error::Unauthorized)?;
		let id = tokens.id_token().ok_or(Error::Unauthorized)?;
		let claims = id
			.claims(&client.id_token_verifier(), &Nonce::new(login.nonce))
			.map_err(|_| Error::Unauthorized)?;
		let provider_subject = claims.subject().as_str().to_owned();
		let active = self.introspect(tokens.access_token().secret()).await?;
		if !active.active
			|| active.sub.as_deref() != Some(provider_subject.as_str())
			|| active.client_id.as_deref() != Some(self.config.client_id.as_str())
		{
			return Err(Error::Unauthorized);
		}
		let account_id = active.account_id.ok_or(Error::Unauthorized)?;

		let token = secret();
		let now = Utc::now();
		let session = Session {
			account_id,
			source_identity_id: active.source_identity_id.ok_or(Error::Unauthorized)?,
			provider_subject,
			csrf_token: secret(),
			refresh_request_id: Uuid::new_v4(),
			access_token: tokens.access_token().secret().clone(),
			refresh_token: tokens.refresh_token().map(|t| t.secret().clone()),
			created_at: now,
			last_used: now,
		};
		self.store
			.connection
			.atomic(async |tx| {
				store::put(
					tx,
					&format!("session:{}", hash(&token)),
					"session",
					&session,
				)
				.await
			})
			.await?;
		let mut response = Response::new(StatusCode::FOUND)
			.with_header("location", &self.config.origin)
			.with_header("cache-control", "no-store");
		response.headers.append(
			"set-cookie",
			self.cookie(self.session_cookie(), &token, SESSION_ABSOLUTE_SECONDS)
				.parse()
				.map_err(|_| Error::Unavailable)?,
		);
		response.headers.append(
			"set-cookie",
			self.cookie(self.login_cookie(), "", 0)
				.parse()
				.map_err(|_| Error::Unavailable)?,
		);
		Ok(response)
	}
	pub(crate) async fn session(&self, request: &Request) -> Result<Response> {
		if self.cookie_value(request, self.session_cookie()).is_none() {
			return json(serde_json::json!({"authenticated":false}));
		}
		let session = self.authenticate(request).await?;
		json(
			serde_json::json!({"authenticated":true,"account_id":session.account_id,"csrf_token":session.csrf_token}),
		)
	}

	pub(crate) async fn logout(&self, request: &Request) -> Result<Response> {
		let token = self
			.cookie_value(request, self.session_cookie())
			.ok_or(Error::Unauthorized)?;
		let key = format!("session:{}", hash(&token));
		// Logging out only withdraws this local capability. It must still work
		// during an authentication outage or after a remote session expires.
		let session: Session = self
			.store
			.connection
			.atomic(async |tx| {
				store::lock_key(tx, &key).await?;
				let session: Session = store::get(tx, &key).await?.ok_or(Error::Unauthorized)?;
				self.csrf(request, &session)?;
				store::remove(tx, &key).await?;
				Ok::<_, Error>(session)
			})
			.await?;
		let revoke = session
			.refresh_token
			.as_ref()
			.unwrap_or(&session.access_token);
		// Local logout is complete even if the authentication dependency is lost
		// after this already-admitted operation. The local session is gone.
		let _ = self
			.http
			.post(format!(
				"{}/oauth/revoke",
				self.config.issuer.trim_end_matches('/')
			))
			.basic_auth(
				oauth_component(&self.config.client_id),
				Some(oauth_component(&self.config.client_secret)),
			)
			.form(&[("token", revoke)])
			.send()
			.await;
		Ok(json(serde_json::json!({"authenticated":false}))?
			.with_header("set-cookie", &self.cookie(self.session_cookie(), "", 0)))
	}
}

/// OAuth client_secret_basic applies application/x-www-form-urlencoded encoding
/// inside the HTTP Basic credential. Resource-server Basic remains RFC 7617.
fn oauth_component(value: &str) -> String {
	url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
