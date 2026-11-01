//! A common OIDC subject identifies an Account, not a local authorization source.
use super::Client;
use crate::{
	Error, Result,
	oidc::{PendingLogin, Settings},
};
use aidash_domain::identity::dashboard::SignIn;
use openidconnect::{
	AuthType, AuthorizationCode, ClientId, ClientSecret, Nonce, OAuth2TokenResponse,
	PkceCodeVerifier, RedirectUrl, TokenResponse,
	core::{CoreClient, CoreProviderMetadata},
};
use serde::Deserialize;
use uuid::Uuid;

pub struct SourceLogin {
	pub identity_id: Uuid,
	pub product_session_id: Uuid,
	pub sign_in: SignIn,
}

#[derive(Deserialize)]
struct UserInfo {
	sub: String,
	account_id: Uuid,
	source_identity_id: Uuid,
	product_session_id: Uuid,
	source_issuer: String,
	source_subject: String,
	#[serde(default)]
	email_verified: bool,
	email: Option<String>,
	name: Option<String>,
}

pub async fn exchange_source(
	config: &Settings,
	metadata: CoreProviderMetadata,
	pending: &PendingLogin<'_>,
	code: String,
	authority: &Client,
) -> Result<SourceLogin> {
	let endpoint = metadata
		.userinfo_endpoint()
		.ok_or(Error::Unauthorized)?
		.url()
		.clone();
	if endpoint.scheme() != "https"
		&& !matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1"))
	{
		return Err(Error::Unauthorized);
	}
	let client = CoreClient::from_provider_metadata(
		metadata,
		ClientId::new(config.client_id.clone()),
		Some(ClientSecret::new(config.client_secret.clone())),
	)
	.set_redirect_uri(
		RedirectUrl::new(pending.callback_uri.to_owned()).map_err(|_| Error::Unauthorized)?,
	)
	.set_auth_type(AuthType::BasicAuth);
	let http = openidconnect::reqwest::Client::builder()
		.redirect(openidconnect::reqwest::redirect::Policy::none())
		.user_agent(crate::USER_AGENT)
		.timeout(std::time::Duration::from_secs(10))
		.build()
		.map_err(|_| Error::IdentityStatusUnavailable)?;
	let tokens = client
		.exchange_code(AuthorizationCode::new(code))
		.map_err(|_| Error::Unauthorized)?
		.set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier.to_owned()))
		.request_async(&http)
		.await
		.map_err(|_| Error::Unauthorized)?;
	let claims = tokens
		.id_token()
		.ok_or(Error::Unauthorized)?
		.claims(
			&client.id_token_verifier(),
			&Nonce::new(pending.nonce.to_owned()),
		)
		.map_err(|_| Error::Unauthorized)?;
	let subject = claims.subject().as_str();
	// Fetch source claims only from the verified issuer's authenticated userinfo
	// endpoint. Neither a browser body nor the common Account subject is a Mapping.
	let mut response = http
		.get(endpoint)
		.bearer_auth(tokens.access_token().secret())
		.send()
		.await
		.map_err(|_| Error::IdentityStatusUnavailable)?;
	if !response.status().is_success() {
		return Err(Error::Unauthorized);
	}
	let mut body = Vec::new();
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|_| Error::IdentityStatusUnavailable)?
	{
		if body.len() + chunk.len() > 65_536 {
			return Err(Error::Unauthorized);
		}
		body.extend_from_slice(&chunk);
	}
	let info: UserInfo = serde_json::from_slice(&body).map_err(|_| Error::Unauthorized)?;
	if info.sub != subject || info.source_issuer.is_empty() || info.source_subject.is_empty() {
		return Err(Error::Unauthorized);
	}
	let status = authority
		.interactive(info.source_identity_id, info.product_session_id)
		.await?;
	if Uuid::parse_str(&status.account_id).ok() != Some(info.account_id)
		|| status.source_issuer != info.source_issuer
		|| status.source_subject != info.source_subject
	{
		return Err(Error::Forbidden);
	}
	Ok(SourceLogin {
		identity_id: info.source_identity_id,
		product_session_id: info.product_session_id,
		sign_in: SignIn {
			issuer: info.source_issuer,
			subject: info.source_subject,
			auth_time: chrono::Utc::now(),
			verified_email: info
				.email
				.filter(|email| info.email_verified && !email.is_empty() && email.len() <= 320),
			display_name: info
				.name
				.filter(|name| !name.is_empty() && name.len() <= 512),
		},
	})
}
