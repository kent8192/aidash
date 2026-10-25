//! First-party browser interaction and OAuth protocol adapters.
use crate::{
	Error, Result,
	service::{Authentication, Flow, digest, secret},
};
use aes_gcm::{
	Aes256Gcm, KeyInit, Nonce,
	aead::{Aead, Payload},
};
use aidash_application::{accounts::Service as Accounts, ports::accounts::Scope};
use aidash_domain::accounts::*;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use http::{Method, StatusCode, header};
use reinhardt::auth::{
	oauth2_server::{OAuthBrowserSession, TokenPrincipal},
	oidc_op::{
		OidcAuthorizationDecision, OidcError, OidcHandler, OidcInteraction, OidcPendingHandle,
	},
};
use reinhardt::{Handler, Request, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

const COOKIE: &str = "__Host-aidash-auth";
/// Two years; browsers ignore the header over plain HTTP, so loopback development omits it.
pub(crate) const HSTS: &str = "max-age=63072000; includeSubDomains";
/// Compares secrets in constant time so response timing does not reveal a matching prefix.
fn same_secret(left: &str, right: &str) -> bool {
	use subtle::ConstantTimeEq;
	bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}
#[derive(Clone)]
pub struct Http {
	auth: Arc<Authentication>,
}
impl Http {
	pub fn new(auth: Arc<Authentication>) -> Self {
		Self { auth }
	}
}
#[async_trait]
impl Handler for Http {
	async fn handle(&self, request: Request) -> reinhardt::core::exception::Result<Response> {
		let mut response = self.respond(request).await?;
		if !self.auth.config.loopback_development {
			response.headers.insert(
				header::STRICT_TRANSPORT_SECURITY,
				HSTS.parse().expect("static header"),
			);
		}
		Ok(response)
	}
}
impl Http {
	async fn respond(&self, request: Request) -> reinhardt::core::exception::Result<Response> {
		let path = request.uri.path().to_owned();
		if matches!(path.as_str(), "/health" | "/healthz") && request.method == Method::GET {
			return Ok(json_response(
				StatusCode::OK,
				json!({"service":"aidash-auth-server"}),
			));
		}
		request.set_trusted_proxies(reinhardt::http::TrustedProxies::new(
			crate::config::settings::trusted_peer(
				&self.auth.config.trusted_proxy_ips,
				request.remote_addr,
			),
		));
		if !self.auth.config.loopback_development && !request.is_secure() {
			return Ok(failure(Error::Invalid));
		}
		let limiter_path = if [
			"/oauth/authorize",
			"/oauth/token",
			"/oauth/userinfo",
			"/oauth/introspect",
			"/oauth/revoke",
			"/login/context",
			"/login/tenant",
			"/login/verify",
			"/login/choice",
			"/account",
			"/account/link/start",
			"/account/unlink/start",
			"/logout",
			"/logout/all",
			"/internal/accounts/check",
		]
		.contains(&path.as_str())
		{
			path.as_str()
		} else {
			"/public"
		};
		let key = format!(
			"http:{}:{}",
			limiter_path,
			request
				.get_client_ip()
				.map_or_else(|| "unknown".into(), |v| v.to_string())
		);
		if !self.auth.oauth.allow(&key).await {
			return Ok(json_response(
				StatusCode::TOO_MANY_REQUESTS,
				json!({"error":"temporarily_unavailable"}),
			));
		}
		let result = self.dispatch(request).await;
		Ok(match result {
			Ok(mut response) => {
				secure_headers(&mut response, Some(&self.auth.config.gcip_auth_domain));
				response
			}
			Err(error) => failure(error),
		})
	}
}
impl Http {
	async fn dispatch(&self, request: Request) -> Result<Response> {
		let path = request.uri.path().to_owned();
		match (request.method.clone(), path.as_str()) {
			(Method::GET, "/.well-known/openid-configuration") => {
				let mut metadata = self.auth.oidc.discovery();
				metadata["revocation_endpoint"] =
					json!(format!("{}/oauth/revoke", self.auth.config.issuer));
				metadata["introspection_endpoint"] =
					json!(format!("{}/oauth/introspect", self.auth.config.issuer));
				Ok(json_response(StatusCode::OK, metadata))
			}
			(Method::GET, "/.well-known/oauth-authorization-server") => {
				Ok(json_response(StatusCode::OK, self.auth.oidc.discovery()))
			}
			(Method::GET, "/oauth/jwks") => {
				Ok(json_response(StatusCode::OK, self.auth.oidc.jwks().await?))
			}
			(Method::GET, "/oauth/authorize") => {
				let browser =
					cookie(&request, self.auth.config.loopback_development).unwrap_or_else(secret);
				request
					.extensions
					.insert(OAuthBrowserSession(browser.clone()));
				let handler = OidcHandler::authorization(
					self.auth.oidc.clone(),
					Arc::new(Interaction(self.auth.clone())),
				);
				let mut response = handler.handle(request).await?;
				set_cookie(
					&mut response,
					&browser,
					self.auth.config.loopback_development,
					43200,
				)?;
				Ok(response)
			}
			(Method::POST, "/oauth/token") => self.token(request).await,
			(Method::GET, "/oauth/userinfo") | (Method::POST, "/oauth/userinfo") => {
				self.userinfo(request).await
			}
			(Method::POST, "/oauth/introspect") => self.introspect(request).await,
			(Method::POST, "/oauth/revoke") => self.revoke(request).await,
			(Method::POST, "/internal/accounts/check") => self.account_status(request).await,
			(Method::GET, "/login/context") => {
				let input = params(request.uri.query().unwrap_or_default())?;
				let id = parse_uuid(input.get("flow").ok_or(Error::Invalid)?)?;
				let flow = self.load_flow(&request, id, None).await?;
				Ok(json_response(StatusCode::OK, self.context(id, &flow)))
			}
			(Method::POST, "/login/tenant") => {
				let input: TenantInput = browser_json(&request, &self.auth)?;
				let mut flow = self
					.load_flow(&request, input.flow, Some(&input.csrf))
					.await?;
				if !self.auth.config.gcip_tenants.contains(&input.tenant_id) {
					return Err(Error::Invalid);
				}
				flow.tenant = input.tenant_id;
				flow.started_at = Utc::now();
				let mut tx = self.auth.store.scope().await?;
				tx.save_flow(input.flow, &browser_digest(&request, &self.auth)?, &flow)
					.await?;
				Box::new(tx).commit().await?;
				Ok(json_response(
					StatusCode::OK,
					self.context(input.flow, &flow),
				))
			}
			(Method::POST, "/login/verify") => self.verify_login(request).await,
			(Method::POST, "/login/choice") => self.choose_account(request).await,
			(Method::GET, "/account") => self.account(request).await,
			(Method::POST, "/account/link/start") => self.account_flow(request, false).await,
			(Method::POST, "/account/unlink/start") => self.account_flow(request, true).await,
			(Method::POST, "/logout") => self.logout(request, false).await,
			(Method::POST, "/logout/all") => self.logout(request, true).await,
			(Method::GET, "/sign-in") | (Method::GET, "/settings/account") | (Method::GET, "/") => {
				self.asset("index.html").await
			}
			(Method::GET, path) if path.starts_with("/assets/") => {
				self.asset(path.trim_start_matches('/')).await
			}
			_ => Ok(json_response(
				StatusCode::NOT_FOUND,
				json!({"error":"not_found"}),
			)),
		}
	}
	async fn asset(&self, relative: &str) -> Result<Response> {
		if relative
			.split('/')
			.any(|part| part == ".." || part.is_empty())
			|| relative.contains('\\')
			|| relative.contains('%')
		{
			return Err(Error::Invalid);
		}
		let base = self
			.auth
			.config
			.assets_dir
			.as_ref()
			.ok_or(Error::Unavailable)?;
		let body = tokio::fs::read(base.join(relative))
			.await
			.map_err(|_| Error::Unavailable)?;
		let content_type = if relative.ends_with(".wasm") {
			"application/wasm"
		} else if relative.ends_with(".js") {
			"text/javascript"
		} else if relative.ends_with(".css") {
			"text/css"
		} else if relative.ends_with(".svg") {
			"image/svg+xml"
		} else if relative.ends_with(".woff2") {
			"font/woff2"
		} else if relative.ends_with(".woff") {
			"font/woff"
		} else if relative.ends_with(".png") {
			"image/png"
		} else if relative.ends_with(".ico") {
			"image/x-icon"
		} else {
			"text/html; charset=utf-8"
		};
		let mut response = Response::new(StatusCode::OK).with_body(bytes::Bytes::from(body));
		response.headers.insert(
			header::CONTENT_TYPE,
			content_type.parse().expect("static MIME"),
		);
		Ok(response)
	}
	fn context(&self, id: Uuid, flow: &Flow) -> Value {
		let phase = if flow.purpose == "link" {
			if flow.proof.is_some() {
				"prove-target"
			} else {
				"prove-current"
			}
		} else if flow.purpose == "unlink" {
			"prove-other"
		} else if flow.target_proof.is_some() {
			"prove-existing"
		} else if flow.proof.is_some() {
			"choose-account"
		} else {
			"sign-in"
		};
		json!({"flow":id,"csrf":flow.csrf,"tenant_ids":self.auth.config.gcip_tenants,"selected_tenant_id":flow.tenant,"client_id":flow.client_id,"phase":phase,"relink_only":flow.account.is_some(),"providers":self.auth.config.gcip_providers.get(&flow.tenant).cloned().unwrap_or_else(||vec!["google.com".into(),"password".into()]),"password_sign_up":self.auth.config.gcip_password_sign_up,"mfa_mandatory":self.auth.config.gcip_mfa_mandatory.contains(&flow.tenant),"firebase":{"project_id":self.auth.config.gcip_project,"api_key":self.auth.config.gcip_browser_api_key,"auth_domain":self.auth.config.gcip_auth_domain}})
	}
	async fn load_flow(&self, request: &Request, id: Uuid, csrf: Option<&str>) -> Result<Flow> {
		let mut tx = self.auth.store.scope().await?;
		let flow: Flow = tx
			.flow(id, &browser_digest(request, &self.auth)?)
			.await?
			.ok_or(Error::Unauthorized)?;
		if flow.consumed
			|| flow.expires_at <= Utc::now()
			|| csrf.is_some_and(|csrf| !same_secret(csrf, &flow.csrf))
		{
			return Err(Error::Unauthorized);
		}
		Ok(flow)
	}
	async fn verify_login(&self, request: Request) -> Result<Response> {
		let input: VerifyInput = browser_json(&request, &self.auth)?;
		let mut flow = self
			.load_flow(&request, input.flow, Some(&input.csrf))
			.await?;
		let (sign_in, provider) = self
			.auth
			.verifier
			.verify(&input.id_token, &flow.tenant, flow.started_at)
			.await?;
		let allowed = self
			.auth
			.config
			.gcip_providers
			.get(&flow.tenant)
			.cloned()
			.unwrap_or_else(|| vec!["google.com".into(), "password".into()]);
		if !allowed.contains(&provider) {
			return Err(Error::Unauthorized);
		}
		let state = self
			.auth
			.upstream
			.lookup(&sign_in.issuer, &sign_in.subject)
			.await?;
		let mut tx = self.auth.store.scope().await?;
		let mut identity = tx
			.identity_by_source(&sign_in.issuer, &sign_in.subject)
			.await?
			.unwrap_or(ExternalIdentity {
				id: Uuid::new_v4(),
				issuer: sign_in.issuer.clone(),
				subject: sign_in.subject.clone(),
				first_account: None,
				linked: false,
				status: IdentityStatus::Enabled,
				disable_epoch: 0,
				last_valid_at: None,
			});
		if state.disabled {
			if identity.status == IdentityStatus::Enabled {
				identity.disable_epoch = identity
					.disable_epoch
					.checked_add(1)
					.ok_or(Error::Unavailable)?;
				identity.status = IdentityStatus::Disabled;
			}
			tx.revoke_identity_sessions(identity.id, Utc::now()).await?;
			tx.save_identity(&identity).await?;
			Box::new(tx).commit().await?;
			return Err(Error::Unauthorized);
		}
		if identity.status == IdentityStatus::Disabled {
			return Err(Error::Forbidden);
		}
		identity.last_valid_at = Some(Utc::now());
		tx.save_identity(&identity).await?;
		tx.observe_source(identity.id, state.valid_since).await?;
		if aidash_domain::identity::dashboard::session_revoked(sign_in.auth_time, state.valid_since)
		{
			Box::new(tx).commit().await?;
			return Err(Error::Unauthorized);
		}
		let proof = FreshProof {
			id: Uuid::new_v4(),
			identity_id: identity.id,
			authenticated_at: sign_in.auth_time,
			expires_at: sign_in.auth_time + Duration::minutes(5),
		};
		tx.put_proof(&proof).await?;
		tx.put_proof_profile(&proof, &sign_in).await?;
		Box::new(tx).commit().await?;
		if flow.purpose == "unlink" {
			let account = flow.account.ok_or(Error::Unauthorized)?;
			Accounts::new(self.auth.store.clone())
				.unlink(
					account,
					flow.unlink_identity.ok_or(Error::Invalid)?,
					proof.id,
					Utc::now(),
				)
				.await?;
			flow.consumed = true;
			self.save_flow(&request, input.flow, &flow).await?;
			return Ok(json_response(
				StatusCode::OK,
				json!({"redirect_to":"/settings/account"}),
			));
		}
		if flow.purpose == "link" {
			if let Some(current) = &flow.proof {
				Accounts::new(self.auth.store.clone())
					.link(
						flow.account.ok_or(Error::Unauthorized)?,
						current.id,
						proof.id,
						Utc::now(),
					)
					.await?;
				flow.consumed = true;
				self.save_flow(&request, input.flow, &flow).await?;
				return Ok(json_response(
					StatusCode::OK,
					json!({"redirect_to":"/settings/account"}),
				));
			}
			if identity.first_account != flow.account || !identity.linked {
				return Err(Error::Unauthorized);
			}
			flow.proof = Some(proof);
			flow.started_at = Utc::now();
			self.save_flow(&request, input.flow, &flow).await?;
			return Ok(json_response(
				StatusCode::OK,
				self.context(input.flow, &flow),
			));
		}
		if flow.target_proof.is_some() {
			let target = flow.proof.clone().ok_or(Error::Unauthorized)?;
			let account = identity
				.first_account
				.filter(|_| identity.linked)
				.ok_or(Error::Unauthorized)?;
			Accounts::new(self.auth.store.clone())
				.link(account, proof.id, target.id, Utc::now())
				.await?;
			return self.finish_login(&request, input.flow, flow, target).await;
		}
		if identity.linked && identity.first_account.is_some() {
			return self.finish_login(&request, input.flow, flow, proof).await;
		}
		flow.account = identity.first_account;
		flow.proof = Some(proof);
		self.save_flow(&request, input.flow, &flow).await?;
		Ok(json_response(
			StatusCode::OK,
			self.context(input.flow, &flow),
		))
	}
	async fn choose_account(&self, request: Request) -> Result<Response> {
		let input: ChoiceInput = browser_json(&request, &self.auth)?;
		let mut flow = self
			.load_flow(&request, input.flow, Some(&input.csrf))
			.await?;
		if flow.purpose != "login" || flow.target_proof.is_some() {
			return Err(Error::Invalid);
		}
		let proof = flow.proof.clone().ok_or(Error::Unauthorized)?;
		match input.choice.as_str() {
			"new" if flow.account.is_none() => {
				Accounts::new(self.auth.store.clone())
					.register(Uuid::new_v4(), proof.id, Utc::now())
					.await?;
				self.finish_login(&request, input.flow, flow, proof).await
			}
			"existing" => {
				flow.target_proof = Some(proof);
				flow.started_at = Utc::now();
				self.save_flow(&request, input.flow, &flow).await?;
				Ok(json_response(
					StatusCode::OK,
					self.context(input.flow, &flow),
				))
			}
			_ => Err(Error::Invalid),
		}
	}
	async fn save_flow(&self, request: &Request, id: Uuid, flow: &Flow) -> Result<()> {
		let mut tx = self.auth.store.scope().await?;
		tx.save_flow(id, &browser_digest(request, &self.auth)?, flow)
			.await?;
		Box::new(tx).commit().await?;
		Ok(())
	}
	async fn finish_login(
		&self,
		request: &Request,
		id: Uuid,
		mut flow: Flow,
		proof: FreshProof,
	) -> Result<Response> {
		let product = self
			.auth
			.issue_session(proof.identity_id, &flow.client_id, proof.authenticated_at)
			.await?;
		let sso = self
			.auth
			.issue_session(proof.identity_id, "sso", proof.authenticated_at)
			.await?;
		let browser =
			cookie(request, self.auth.config.loopback_development).ok_or(Error::Unauthorized)?;
		let location = self
			.auth
			.oidc
			.complete_authorization(
				&flow.pending,
				&browser,
				OidcAuthorizationDecision::Approve {
					user_id: product.account_id.to_string(),
					auth_time: proof.authenticated_at.timestamp(),
					consented: true,
					consent_prompted: true,
					account_selected: true,
					reauthenticated: true,
				},
			)
			.await?;
		let code = url::Url::parse(&location)
			.map_err(|_| Error::Unavailable)?
			.query_pairs()
			.find(|(key, _)| key == "code")
			.map(|(_, v)| v.into_owned())
			.ok_or(Error::Unavailable)?;
		flow.consumed = true;
		let mut tx = self.auth.store.scope().await?;
		tx.copy_profile(proof.id, "proof", &product).await?;
		tx.copy_profile(proof.id, "proof", &sso).await?;
		tx.bind(digest(&code), product.id, "code").await?;
		tx.bind(format!("sso:{}", digest(&browser)), sso.id, "sso")
			.await?;
		tx.save_flow(id, &digest(&browser), &flow).await?;
		Box::new(tx).commit().await?;
		Ok(json_response(
			StatusCode::OK,
			json!({"redirect_to":location}),
		))
	}
	async fn sso(&self, request: &Request) -> Result<(ProductSession, ExternalIdentity)> {
		let browser = browser_digest(request, &self.auth)?;
		let mut tx = self.auth.store.scope().await?;
		let id = tx
			.binding(format!("sso:{browser}"), "sso")
			.await?
			.ok_or(Error::Unauthorized)?;
		drop(tx);
		let (validity, session, identity) = self.auth.check_session(id).await?;
		if validity != Validity::Active {
			return Err(Error::Unauthorized);
		}
		Ok((
			session.ok_or(Error::Unauthorized)?,
			identity.ok_or(Error::Unauthorized)?,
		))
	}
	async fn account(&self, request: Request) -> Result<Response> {
		let (session, source) = self.sso(&request).await?;
		use reinhardt::db::orm::Model;
		let mut tx = self.auth.store.scope().await?;
		let records = crate::models::IdentityRecord::objects()
			.all()
			.all_with_executor(tx.executor())
			.await
			.map_err(crate::store::unavailable)?;
		let mut identities = Vec::new();
		for record in records {
			let identity: ExternalIdentity = crate::store::decode(record.document.0)?;
			if identity.first_account == Some(session.account_id) {
				identities.push(json!({"id":identity.id,"issuer":identity.issuer,"subject":identity.subject,"linked":identity.linked}));
			}
		}
		// Browser-specific secret supplies CSRF protection without a readable auth cookie.
		let csrf = digest(&format!(
			"account:{}",
			cookie(&request, self.auth.config.loopback_development).ok_or(Error::Unauthorized)?
		));
		Ok(json_response(
			StatusCode::OK,
			json!({"account_id":session.account_id,"source_identity_id":source.id,"csrf":csrf,"identities":identities}),
		))
	}
	async fn account_flow(&self, request: Request, unlink: bool) -> Result<Response> {
		let input: AccountInput = browser_json(&request, &self.auth)?;
		self.account_csrf(&request, &input.csrf)?;
		let (session, _) = self.sso(&request).await?;
		if unlink && input.identity_id.is_none() {
			return Err(Error::Invalid);
		}
		let now = Utc::now();
		let id = Uuid::new_v4();
		let flow = Flow {
			pending: String::new(),
			client_id: "sso".into(),
			csrf: secret(),
			tenant: self.auth.config.gcip_tenants[0].clone(),
			started_at: now,
			expires_at: now + Duration::minutes(10),
			proof: None,
			target_proof: None,
			consumed: false,
			purpose: if unlink { "unlink" } else { "link" }.into(),
			account: Some(session.account_id),
			unlink_identity: input.identity_id,
		};
		self.save_flow(&request, id, &flow).await?;
		Ok(json_response(
			StatusCode::OK,
			json!({"redirect_to":format!("/sign-in?flow={id}")}),
		))
	}
	fn account_csrf(&self, request: &Request, csrf: &str) -> Result<()> {
		let expected = digest(&format!(
			"account:{}",
			cookie(request, self.auth.config.loopback_development).ok_or(Error::Unauthorized)?
		));
		if !same_secret(csrf, &expected) {
			return Err(Error::Unauthorized);
		}
		Ok(())
	}
	async fn logout(&self, request: Request, all: bool) -> Result<Response> {
		let input: AccountInput = browser_json(&request, &self.auth)?;
		self.account_csrf(&request, &input.csrf)?;
		let (session, _) = self.sso(&request).await?;
		if all {
			Accounts::new(self.auth.store.clone())
				.logout_all_authenticated(session.id, Utc::now())
				.await?;
		} else {
			Accounts::new(self.auth.store.clone())
				.logout(session.id, Utc::now())
				.await?;
		}
		let mut response = json_response(StatusCode::OK, json!({"completed":true}));
		set_cookie(&mut response, "", self.auth.config.loopback_development, 0)?;
		Ok(response)
	}
	async fn token(&self, request: Request) -> Result<Response> {
		let input = form(&request)?;
		let (client, client_secret) = client_basic(&request)?;
		if input.contains_key("client_id") || input.contains_key("client_secret") {
			return Err(Error::Invalid);
		}
		let grant = input.get("grant_type").ok_or(Error::Invalid)?;
		match grant.as_str() {
			"authorization_code" => {
				let code = input.get("code").ok_or(Error::Invalid)?;
				let mut tx = self.auth.store.scope().await?;
				let session = tx
					.binding(digest(code), "code")
					.await?
					.ok_or(Error::Unauthorized)?;
				drop(tx);
				let (validity, session, _) = self.auth.check_session(session).await?;
				let session = session
					.filter(|s| s.client_id == client)
					.ok_or(Error::Unauthorized)?;
				require_active(validity)?;
				let response = self
					.auth
					.oidc
					.exchange_code(
						code,
						&client,
						&client_secret,
						input.get("redirect_uri").ok_or(Error::Invalid)?,
						input.get("code_verifier").ok_or(Error::Invalid)?,
					)
					.await?;
				let mut tx = self.auth.store.scope().await?;
				tx.bind(digest(&response.access_token), session.id, "access")
					.await?;
				if let Some(refresh) = &response.refresh_token {
					tx.bind(digest(refresh), session.id, "refresh").await?;
				}
				Box::new(tx).commit().await?;
				Ok(json_response(
					StatusCode::OK,
					serde_json::to_value(response).map_err(|_| Error::Unavailable)?,
				))
			}
			"refresh_token" => {
				if input.get("scope").is_some_and(|v| v != "openid")
					|| input.contains_key("resource")
				{
					return Err(Error::Invalid);
				}
				self.refresh(
					&request,
					&client,
					&client_secret,
					input.get("refresh_token").ok_or(Error::Invalid)?,
				)
				.await
			}
			_ => Ok(json_response(
				StatusCode::BAD_REQUEST,
				json!({"error":"unsupported_grant_type"}),
			)),
		}
	}
	async fn refresh(
		&self,
		request: &Request,
		client: &str,
		secret: &str,
		refresh: &str,
	) -> Result<Response> {
		self.auth.authenticate_client(client, secret).await?;
		let request_id = request
			.headers
			.get("idempotency-key")
			.and_then(|v| v.to_str().ok())
			.ok_or(Error::Invalid)?;
		let id = Uuid::parse_str(request_id).map_err(|_| Error::Invalid)?;
		if id.get_version_num() != 4 {
			return Err(Error::Invalid);
		}
		let token_digest = digest(refresh);
		let lock = self.auth.store.refresh_lock(&token_digest).await?;
		let mut tx = self.auth.store.scope().await?;
		let session = tx
			.binding(token_digest.clone(), "refresh")
			.await?
			.ok_or(Error::Unauthorized)?;
		let cached: Option<CachedRefresh> = tx.refresh_response(&token_digest).await?;
		drop(tx);
		let (validity, bound, _) = self.auth.check_session(session).await?;
		require_active(validity)?;
		let bound = bound
			.filter(|s| s.client_id == client)
			.ok_or(Error::Unauthorized)?;
		if let Some(cached) = cached.filter(|c| {
			c.expires_at > Utc::now().timestamp()
				&& c.request_digest == digest(request_id)
				&& c.client == client
				&& c.session == session
		}) {
			let response = cached.decrypt(&self.auth)?;
			let access = response
				.get("access_token")
				.and_then(Value::as_str)
				.ok_or(Error::Unavailable)?;
			if self.auth.oauth.token_info(access).await?.is_none() {
				return Err(Error::Unauthorized);
			}
			lock.commit().await.map_err(crate::store::unavailable)?;
			return Ok(json_response(StatusCode::OK, response));
		}
		let response = match self.auth.oidc.refresh(refresh, client, secret).await {
			Ok(response) => response,
			Err(error) => {
				// A confirmed invalid/reused family never gets another browser
				// session substituted. Scope the stop to this product/device.
				if error == OidcError::InvalidGrant {
					Accounts::new(self.auth.store.clone())
						.logout(session, Utc::now())
						.await?;
				}
				return Err(error.into());
			}
		};
		let wire = serde_json::to_value(&response).map_err(|_| Error::Unavailable)?;
		let cached = CachedRefresh::encrypt(&self.auth, client, bound.id, request_id, &wire)?;
		let mut tx = self.auth.store.scope().await?;
		tx.bind(digest(&response.access_token), session, "access")
			.await?;
		if let Some(refresh) = &response.refresh_token {
			tx.bind(digest(refresh), session, "refresh").await?;
		}
		tx.save_refresh_response(&token_digest, &cached).await?;
		Box::new(tx).commit().await?;
		lock.commit().await.map_err(crate::store::unavailable)?;
		Ok(json_response(StatusCode::OK, wire))
	}
	async fn token_claims(&self, token: &str, client: Option<&str>) -> Result<Option<Value>> {
		let Some(mut claims) = self.auth.oidc.userinfo(token).await? else {
			return Ok(None);
		};
		let Some(info) = self.auth.oauth.token_info(token).await? else {
			return Ok(None);
		};
		if client.is_some_and(|id| id != info.client_id) {
			return Ok(None);
		}
		let mut tx = self.auth.store.scope().await?;
		let session = tx.binding(digest(token), "access").await?;
		drop(tx);
		let Some(session) = session else {
			return Ok(None);
		};
		let (validity, session, source) = self.auth.check_session(session).await?;
		match validity {
			Validity::Active => {}
			Validity::Unavailable => return Err(Error::Unavailable),
			_ => return Ok(None),
		}
		let session = session.ok_or(Error::Unavailable)?;
		let source = source.ok_or(Error::Unavailable)?;
		if info.principal != TokenPrincipal::User(session.account_id.to_string())
			|| info.client_id != session.client_id
		{
			return Ok(None);
		}
		claims["active"] = json!(true);
		claims["account_id"] = json!(session.account_id);
		claims["source_identity_id"] = json!(source.id);
		claims["product_session_id"] = json!(session.id);
		claims["client_id"] = json!(session.client_id);
		claims["source_issuer"] = json!(source.issuer);
		claims["source_subject"] = json!(source.subject);
		claims["exp"] = json!(info.expires_at);
		claims["auth_time"] = json!(session.issued_at.timestamp());
		// Display data belongs to the exact authenticated source and this
		// session, never to the Account or the union of its linked methods.
		let mut tx = self.auth.store.scope().await?;
		if let Some(profile) = tx.session_profile(&session).await? {
			if let Some(email) = profile.verified_email {
				claims["email"] = json!(email);
				claims["email_verified"] = json!(true);
			}
			if let Some(name) = profile.display_name {
				claims["name"] = json!(name);
			}
		}
		Ok(Some(claims))
	}
	async fn userinfo(&self, request: Request) -> Result<Response> {
		let token = request
			.headers
			.get(header::AUTHORIZATION)
			.and_then(|v| v.to_str().ok())
			.and_then(|v| v.strip_prefix("Bearer "))
			.filter(|v| !v.is_empty())
			.ok_or(Error::Unauthorized)?;
		let claims = self
			.token_claims(token, None)
			.await?
			.ok_or(Error::Unauthorized)?;
		Ok(json_response(StatusCode::OK, claims))
	}
	async fn introspect(&self, request: Request) -> Result<Response> {
		let (resource, secret) = basic(&request)?;
		let client = self.auth.resource_client(&resource, &secret)?;
		let input = form(&request)?;
		let token = input.get("token").ok_or(Error::Invalid)?;
		let claims = self
			.token_claims(token, Some(&client))
			.await?
			.unwrap_or(json!({"active":false}));
		Ok(json_response(StatusCode::OK, claims))
	}
	async fn revoke(&self, request: Request) -> Result<Response> {
		let (client, secret) = client_basic(&request)?;
		self.auth.authenticate_client(&client, &secret).await?;
		let input = form(&request)?;
		let token = input.get("token").ok_or(Error::Invalid)?;
		self.auth
			.oauth
			.revoke_with_hint(
				token,
				&client,
				Some(&secret),
				input.get("token_type_hint").map(String::as_str),
			)
			.await?;
		let mut tx = self.auth.store.scope().await?;
		let session = if let Some(session) = tx.binding(digest(token), "refresh").await? {
			Some(session)
		} else {
			tx.binding(digest(token), "access").await?
		};
		if let Some(id) = session
			&& let Some(session) = tx.session(id).await?
			&& session.client_id == client
		{
			drop(tx);
			Accounts::new(self.auth.store.clone())
				.logout(id, Utc::now())
				.await?;
		}
		Ok(json_response(StatusCode::OK, json!({})))
	}
	async fn account_status(&self, request: Request) -> Result<Response> {
		let (resource, secret) = basic(&request)?;
		let client = self.auth.resource_client(&resource, &secret)?;
		if client != "marketplace" {
			return Err(Error::Forbidden);
		}
		let value: AccountStatusInput = machine_json(&request)?;
		let mut tx = self.auth.store.scope().await?;
		let account = tx.account(value.account_id).await?;
		Ok(json_response(
			StatusCode::OK,
			json!({"active":account.is_some_and(|a| a.status == AccountStatus::Active)}),
		))
	}
}
struct Interaction(Arc<Authentication>);
#[async_trait]
impl OidcInteraction for Interaction {
	async fn present(
		&self,
		request: Request,
		pending: OidcPendingHandle,
	) -> reinhardt::core::exception::Result<Response> {
		let http = Http::new(self.0.clone());
		let operation: Result<Response> = async {
			let force = pending
				.request
				.prompts
				.iter()
				.any(|p| p == "login" || p == "select_account" || p == "consent");
			if !force && let Ok((sso, source)) = http.sso(&request).await {
				let age = Utc::now()
					.timestamp()
					.saturating_sub(sso.issued_at.timestamp());
				if pending
					.request
					.max_age
					.is_none_or(|limit| age <= limit as i64)
				{
					// SSO continues authentication without creating a fresh
					// lifecycle proof; Link changes always use upstream UI.
					let product = self
						.0
						.issue_session(source.id, &pending.request.client_id, sso.issued_at)
						.await?;
					let browser = cookie(&request, self.0.config.loopback_development)
						.ok_or(Error::Unauthorized)?;
					let location = self
						.0
						.oidc
						.complete_authorization(
							&pending.id,
							&browser,
							OidcAuthorizationDecision::Approve {
								user_id: product.account_id.to_string(),
								auth_time: sso.issued_at.timestamp(),
								consented: true,
								consent_prompted: false,
								account_selected: false,
								reauthenticated: false,
							},
						)
						.await?;
					let code = url::Url::parse(&location)
						.map_err(|_| Error::Unavailable)?
						.query_pairs()
						.find(|(key, _)| key == "code")
						.map(|(_, v)| v.into_owned())
						.ok_or(Error::Unavailable)?;
					let mut tx = self.0.store.scope().await?;
					tx.copy_profile(sso.id, "session", &product).await?;
					tx.bind(digest(&code), product.id, "code").await?;
					Box::new(tx).commit().await?;
					return redirect(&location);
				}
			}
			if pending.request.prompts.iter().any(|p| p == "none") {
				let browser = cookie(&request, self.0.config.loopback_development)
					.ok_or(Error::Unauthorized)?;
				let location = self
					.0
					.oidc
					.complete_authorization(
						&pending.id,
						&browser,
						OidcAuthorizationDecision::Deny(OidcError::LoginRequired),
					)
					.await?;
				return redirect(&location);
			}
			let id = Uuid::new_v4();
			let now = Utc::now();
			let flow = Flow {
				pending: pending.id,
				client_id: pending.request.client_id,
				csrf: secret(),
				tenant: self.0.config.gcip_tenants[0].clone(),
				started_at: now,
				expires_at: now + Duration::minutes(10),
				proof: None,
				target_proof: None,
				consumed: false,
				purpose: "login".into(),
				account: None,
				unlink_identity: None,
			};
			let browser = request
				.extensions
				.get::<OAuthBrowserSession>()
				.ok_or(Error::Unauthorized)?
				.0
				.clone();
			let mut tx = self.0.store.scope().await?;
			tx.save_flow(id, &digest(&browser), &flow).await?;
			Box::new(tx).commit().await?;
			redirect(&format!("/sign-in?flow={id}"))
		}
		.await;
		Ok(operation.unwrap_or_else(failure))
	}
}
#[derive(Serialize, Deserialize)]
struct CachedRefresh {
	client: String,
	session: Uuid,
	request_digest: String,
	expires_at: i64,
	nonce: String,
	encrypted: String,
}
impl CachedRefresh {
	fn aad(&self) -> String {
		format!(
			"aidash-auth-refresh-v1:{}:{}:{}",
			self.client, self.session, self.request_digest
		)
	}
	fn encrypt(
		auth: &Authentication,
		client: &str,
		session: Uuid,
		request_id: &str,
		wire: &Value,
	) -> Result<Self> {
		let random = Uuid::new_v4();
		let nonce = &random.as_bytes()[..12];
		let mut value = Self {
			client: client.into(),
			session,
			request_digest: digest(request_id),
			expires_at: Utc::now().timestamp() + 30,
			nonce: STANDARD.encode(nonce),
			encrypted: String::new(),
		};
		let cipher = Aes256Gcm::new_from_slice(auth.encryption_key.as_ref())
			.map_err(|_| Error::Configuration)?;
		let plaintext = serde_json::to_vec(wire).map_err(|_| Error::Unavailable)?;
		value.encrypted = STANDARD.encode(
			cipher
				.encrypt(
					Nonce::from_slice(nonce),
					Payload {
						msg: &plaintext,
						aad: value.aad().as_bytes(),
					},
				)
				.map_err(|_| Error::Unavailable)?,
		);
		Ok(value)
	}
	fn decrypt(&self, auth: &Authentication) -> Result<Value> {
		let nonce = STANDARD
			.decode(&self.nonce)
			.map_err(|_| Error::Unavailable)?;
		if nonce.len() != 12 {
			return Err(Error::Unavailable);
		}
		let bytes = STANDARD
			.decode(&self.encrypted)
			.map_err(|_| Error::Unavailable)?;
		let cipher = Aes256Gcm::new_from_slice(auth.encryption_key.as_ref())
			.map_err(|_| Error::Configuration)?;
		let plaintext = cipher
			.decrypt(
				Nonce::from_slice(&nonce),
				Payload {
					msg: &bytes,
					aad: self.aad().as_bytes(),
				},
			)
			.map_err(|_| Error::Unavailable)?;
		serde_json::from_slice(&plaintext).map_err(|_| Error::Unavailable)
	}
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TenantInput {
	flow: Uuid,
	csrf: String,
	tenant_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyInput {
	flow: Uuid,
	csrf: String,
	id_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChoiceInput {
	flow: Uuid,
	csrf: String,
	choice: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountInput {
	csrf: String,
	#[serde(default)]
	identity_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountStatusInput {
	account_id: Uuid,
}
fn parse_uuid(value: &str) -> Result<Uuid> {
	Uuid::parse_str(value).map_err(|_| Error::Invalid)
}
fn cookie(request: &Request, development: bool) -> Option<String> {
	let name = if development { "aidash-auth" } else { COOKIE };
	let mut values = request
		.headers
		.get_all(header::COOKIE)
		.iter()
		.filter_map(|v| v.to_str().ok())
		.flat_map(|v| v.split(';'))
		.filter_map(|v| v.trim().split_once('='))
		.filter(|(key, _)| *key == name)
		.map(|(_, v)| v);
	let value = values.next()?;
	if values.next().is_some() || value.len() != 64 || !value.bytes().all(|v| v.is_ascii_hexdigit())
	{
		return None;
	}
	Some(value.into())
}
fn browser_digest(request: &Request, auth: &Authentication) -> Result<String> {
	cookie(request, auth.config.loopback_development)
		.map(|v| digest(&v))
		.ok_or(Error::Unauthorized)
}
fn set_cookie(response: &mut Response, value: &str, development: bool, max_age: i64) -> Result<()> {
	let name = if development { "aidash-auth" } else { COOKIE };
	let cookie = format!(
		"{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
		if development { "" } else { "; Secure" }
	);
	response.headers.append(
		header::SET_COOKIE,
		cookie.parse().map_err(|_| Error::Unavailable)?,
	);
	Ok(())
}
fn basic(request: &Request) -> Result<(String, String)> {
	let raw = request
		.headers
		.get(header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.strip_prefix("Basic "))
		.ok_or(Error::Unauthorized)?;
	let raw = STANDARD.decode(raw).map_err(|_| Error::Unauthorized)?;
	let raw = std::str::from_utf8(&raw).map_err(|_| Error::Unauthorized)?;
	let (id, secret) = raw.split_once(':').ok_or(Error::Unauthorized)?;
	if id.is_empty() || secret.is_empty() || raw.len() > 8192 {
		return Err(Error::Unauthorized);
	}
	Ok((id.to_owned(), secret.to_owned()))
}
fn client_basic(request: &Request) -> Result<(String, String)> {
	let (id, secret) = basic(request)?;
	let decode = |v: &str| {
		url::form_urlencoded::parse(format!("v={v}").as_bytes())
			.next()
			.map(|(_, value)| value.into_owned())
			.ok_or(Error::Unauthorized)
	};
	Ok((decode(&id)?, decode(&secret)?))
}
fn params(value: &str) -> Result<HashMap<String, String>> {
	if value.len() > 16384 {
		return Err(Error::Invalid);
	}
	let mut result = HashMap::new();
	for (key, value) in url::form_urlencoded::parse(value.as_bytes()) {
		if result
			.insert(key.into_owned(), value.into_owned())
			.is_some()
		{
			return Err(Error::Invalid);
		}
	}
	Ok(result)
}
fn form(request: &Request) -> Result<HashMap<String, String>> {
	if request
		.headers
		.get(header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| v.split(';').next())
		.map(str::trim)
		!= Some("application/x-www-form-urlencoded")
	{
		return Err(Error::Invalid);
	}
	params(std::str::from_utf8(request.body()).map_err(|_| Error::Invalid)?)
}
fn machine_json<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T> {
	if request.body().len() > 32768
		|| request
			.headers
			.get(header::CONTENT_TYPE)
			.and_then(|v| v.to_str().ok())
			.and_then(|v| v.split(';').next())
			.map(str::trim)
			!= Some("application/json")
	{
		return Err(Error::Invalid);
	}
	serde_json::from_slice(request.body()).map_err(|_| Error::Invalid)
}
fn browser_json<T: serde::de::DeserializeOwned>(
	request: &Request,
	auth: &Authentication,
) -> Result<T> {
	if request
		.headers
		.get(header::ORIGIN)
		.and_then(|v| v.to_str().ok())
		!= Some(auth.config.issuer.as_str())
	{
		return Err(Error::Forbidden);
	}
	machine_json(request)
}
fn require_active(validity: Validity) -> Result<()> {
	match validity {
		Validity::Active => Ok(()),
		Validity::Unavailable => Err(Error::Unavailable),
		_ => Err(Error::Unauthorized),
	}
}
fn redirect(location: &str) -> Result<Response> {
	let mut response = Response::new(StatusCode::FOUND);
	response.headers.insert(
		header::LOCATION,
		location.parse().map_err(|_| Error::Unavailable)?,
	);
	Ok(response)
}
fn json_response(status: StatusCode, value: Value) -> Response {
	let mut response = Response::new(status).with_body(bytes::Bytes::from(
		serde_json::to_vec(&value).expect("JSON values serialize"),
	));
	response.headers.insert(
		header::CONTENT_TYPE,
		"application/json".parse().expect("static MIME"),
	);
	secure_headers(&mut response, None);
	response
}
fn secure_headers(response: &mut Response, auth_domain: Option<&str>) {
	for (name, value) in [
		(header::CACHE_CONTROL, "no-store"),
		(header::REFERRER_POLICY, "no-referrer"),
		(header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
	] {
		response
			.headers
			.insert(name, value.parse().expect("static header"));
	}
	let auth_frame = auth_domain
		.map(|domain| format!(" https://{domain}"))
		.unwrap_or_default();
	// `apis.google.com` and `style-src-attr` stay for Firebase's popup/redirect
	// resolver (gcip-sdk.js), which loads gapi and styles its hidden iframe inline.
	// `<style>` elements and foreign stylesheets remain blocked.
	response.headers.insert(header::CONTENT_SECURITY_POLICY, format!("default-src 'self'; script-src 'self' 'wasm-unsafe-eval' https://apis.google.com; connect-src 'self' https://*.googleapis.com https://*.firebaseapp.com https://*.gstatic.com; frame-src https://*.firebaseapp.com https://accounts.google.com{auth_frame}; style-src 'self'; style-src-attr 'unsafe-inline'; object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'").parse().expect("validated CSP"));
}
fn failure(error: Error) -> Response {
	let (status, code) = match error {
		Error::Accounts(aidash_application::accounts::Error::Rule(
			aidash_domain::accounts::AccountError::LastLoginMethod,
		)) => (StatusCode::CONFLICT, "last_login_method"),
		Error::Accounts(aidash_application::accounts::Error::Rule(_)) => {
			(StatusCode::FORBIDDEN, "access_denied")
		}
		Error::Application(aidash_application::Error::Unauthorized) => {
			(StatusCode::UNAUTHORIZED, "invalid_token")
		}
		Error::Application(aidash_application::Error::Forbidden) => {
			(StatusCode::FORBIDDEN, "access_denied")
		}
		Error::Unauthorized => (StatusCode::UNAUTHORIZED, "invalid_token"),
		Error::Forbidden => (StatusCode::FORBIDDEN, "access_denied"),
		Error::Invalid => (StatusCode::BAD_REQUEST, "invalid_request"),
		Error::Oidc(OidcError::InvalidClient) => (StatusCode::UNAUTHORIZED, "invalid_client"),
		Error::OAuth(reinhardt::auth::oauth2_server::OAuthError::InvalidClient) => {
			(StatusCode::UNAUTHORIZED, "invalid_client")
		}
		Error::Oidc(error) => (StatusCode::BAD_REQUEST, error.as_str()),
		Error::OAuth(error) => (StatusCode::BAD_REQUEST, error.as_str()),
		_ => (StatusCode::SERVICE_UNAVAILABLE, "temporarily_unavailable"),
	};
	let mut response = json_response(status, json!({"error":code}));
	if status == StatusCode::UNAUTHORIZED {
		response.headers.insert(
			header::WWW_AUTHENTICATE,
			if code == "invalid_client" {
				"Basic realm=\"aidash-auth\""
			} else {
				"Bearer error=\"invalid_token\""
			}
			.parse()
			.expect("static challenge"),
		);
	}
	response
}
