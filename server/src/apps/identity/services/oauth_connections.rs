//! Browser OAuth orchestration, ownership and live policy checks (ADR 0100).
use super::super::{
	models::oauth_connections::OAuthAuthorization,
	serializers::oauth_connections::{CallbackQuery, Probe, Start, Started},
};
use super::oidc::{self, BrowserOrigin};
use crate::{
	Error, Result,
	authorization::{access::NativeAccess, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	oauth_connections::{Disconnection, Metadata, Outcome, Owner, Service},
	tenancy::TenancyScope,
};
use aidash_domain::identity::tenant_administration::COLLECTION;
use http::{HeaderMap, Method, StatusCode};
use reinhardt::{Response, injectable};
use secrecy::ExposeSecret;
use std::sync::Arc;
use uuid::Uuid;
const COOKIE: &str = "__Host-aidash-connection-authorization";
#[derive(Clone)]
pub struct Connections {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> Connections {
	Connections { runtime }
}
fn digest(value: &str) -> String {
	oidc::digest(value)
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}
fn dashboard_path(value: Option<&str>) -> Result<&str> {
	let path = oidc::return_path(value)?;
	// Browsers remove tabs while parsing URLs: `/\t/host` would become `//host`.
	if path.chars().any(char::is_control) {
		return Err(Error::Invalid("invalid return destination".into()));
	}
	Ok(path)
}
fn owner(actor: &Actor, origin: Option<&BrowserOrigin>, tenant: &str) -> Result<Owner> {
	match actor {
		Actor::Subject(i)
			if i.tenant == tenant && origin.is_some_and(|o| o.mapping_id.is_some()) =>
		{
			Ok(Owner {
				tenant: i.tenant.clone(),
				subject: i.subject.clone(),
			})
		}
		_ => Err(Error::Forbidden),
	}
}
impl Connections {
	fn service(&self) -> Result<&Arc<Service>> {
		self.runtime
			.store
			.oauth_connections
			.as_ref()
			.ok_or_else(|| {
				Error::NotFound("OAuth Connections are unavailable on this Deployment".into())
			})
	}
	async fn personal_owner(&self, actor: &Actor, tenant: &str) -> Result<(Owner, NativeAccess)> {
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		if identity.tenant != tenant {
			return Err(Error::Forbidden);
		}
		let access = NativeAccess::begin(&self.runtime.store, identity).await?;
		if access
			.snapshot
			.bundle
			.subjects
			.get(&identity.subject)
			.is_none_or(|s| s.kind != aidash_domain::policy::SubjectKind::User)
		{
			return access.finish(Err(Error::Forbidden)).await;
		}
		Ok((
			Owner {
				tenant: tenant.into(),
				subject: identity.subject.clone(),
			},
			access,
		))
	}
	pub(crate) async fn list(
		&self,
		actor: Actor,
		_origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<Vec<Metadata>> {
		let (owner, access) = self.personal_owner(&actor, &tenant).await?;
		access
			.finish(self.service()?.list(&owner).await.map_err(Into::into))
			.await
	}
	pub(crate) async fn disconnect(
		&self,
		actor: Actor,
		_origin: Option<BrowserOrigin>,
		tenant: String,
		id: Uuid,
	) -> Result<Disconnection> {
		let (owner, access) = self.personal_owner(&actor, &tenant).await?;
		access
			.finish(
				self.service()?
					.disconnect(&owner, id)
					.await
					.map_err(Into::into),
			)
			.await
	}
	async fn authorize(
		&self,
		actor: &Actor,
		tenant: &str,
		subject: &str,
		resource_type: &str,
		actions: &[&str],
	) -> Result<NativeAccess> {
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		if identity.tenant != tenant {
			return Err(Error::Forbidden);
		}
		let mut access = NativeAccess::begin(&self.runtime.store, identity).await?;
		let resource = access.resource(
			resource_type,
			subject,
			serde_json::json!({"subject":subject}),
		);
		let mut permitted = false;
		for action in actions {
			if access.decide(&resource, action).await? {
				permitted = true;
				break;
			}
		}
		if !permitted {
			return access.finish(Err(Error::Forbidden)).await;
		}
		Ok(access)
	}
	pub(crate) async fn members(
		&self,
		actor: Actor,
		tenant: String,
		subject: String,
	) -> Result<Vec<Metadata>> {
		let access = self
			.authorize(
				&actor,
				&tenant,
				&subject,
				"connection",
				&["connection.administer"],
			)
			.await?;
		let result = self.service()?.list(&Owner { tenant, subject }).await;
		access.finish(result.map_err(Into::into)).await
	}
	pub(crate) async fn force_disconnect(
		&self,
		actor: Actor,
		tenant: String,
		subject: String,
		id: Uuid,
	) -> Result<Disconnection> {
		let access = self
			.authorize(
				&actor,
				&tenant,
				&subject,
				"connection",
				&["connection.administer"],
			)
			.await?;
		let result = self
			.service()?
			.force_disconnect(&Owner { tenant, subject }, id)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
	pub(crate) async fn probe(
		&self,
		actor: Actor,
		tenant: String,
		resource: String,
	) -> Result<Probe> {
		let Actor::Subject(identity) = &actor else {
			return Err(Error::Forbidden);
		};
		if identity.tenant != tenant {
			return Err(Error::Forbidden);
		}
		let mut access = NativeAccess::begin(&self.runtime.store, identity).await?;
		let registry = access.resource(
			"registry",
			format!("tool/{COLLECTION}"),
			serde_json::json!({"kind":"tool"}),
		);
		let secret = access.resource("secret", COLLECTION, serde_json::json!({"name":COLLECTION}));
		if !access.decide(&registry, "registry.create").await?
			&& !access.decide(&secret, "secret.use").await?
		{
			return access.finish(Err(Error::Forbidden)).await;
		}
		let result: Result<Probe> = async {
			// Probe the configured URL as given; the ceiling is keyed by the canonical resource.
			let canonical = aidash_domain::oauth_connections::canonical_resource(&resource)?;
			let service = self.service()?;
			let probe = service.probe(&resource).await?;
			let scope_ceiling = service.policy.scope_ceiling(&tenant, &canonical).await?;
			Ok(Probe {
				oauth: probe.oauth,
				scopes_supported: probe.scopes_supported,
				issuer: probe.issuer,
				scope_ceiling,
			})
		}
		.await;
		access.finish(result).await
	}
	pub(crate) async fn start(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		headers: HeaderMap,
		tenant: String,
		input: Start,
	) -> Result<Response> {
		let f = &self.runtime;
		let owner = owner(&actor, origin.as_ref(), &tenant)?;
		let origin = origin.ok_or(Error::Forbidden)?;
		let config = oidc::required_config(f)?;
		let (_, access) = self.personal_owner(&actor, &tenant).await?;
		access.finish(Ok(())).await?;
		// Desktop sessions and Subject Credentials cannot start this browser flow.
		let session = oidc::browser_session_from_headers(f, &headers).await?;
		if session.id != origin.session.id {
			return Err(Error::Forbidden);
		}
		let context = headers
			.get("x-aidash-context")
			.and_then(|v| v.to_str().ok())
			.ok_or(Error::Forbidden)?
			.to_owned();
		let return_to = dashboard_path(
			input
				.return_to
				.as_deref()
				.or(Some("/settings?view=connections")),
		)?
		.to_owned();
		let redirect = reqwest::Url::parse(&format!(
			"{}/auth/connections/callback",
			config.public_origin
		))
		.map_err(|_| Error::Invalid("invalid callback origin".into()))?;
		let browser = oidc::cookie_value(&headers, &oidc::cookie_name(COOKIE, config))
			.map(str::to_owned)
			.unwrap_or_else(oidc::random_secret);
		let mut lookup = super::super::repositories::oauth_connections::ToolLookup {
			pool: f.store.control_pool.clone(),
			tenant: tenant.clone(),
		};
		let request = self
			.service()?
			.begin(
				&owner,
				&input.resource,
				&input.scopes,
				input.tool.as_ref(),
				&mut lookup,
				&redirect,
				&return_to,
			)
			.await?;
		let p = request.pending;
		OAuthAuthorization::build()
			.state_hash(digest(request.state.expose_secret()))
			.browser_hash(digest(&browser))
			.session_id(session.id)
			.context(context)
			.tenant(tenant)
			.subject(owner.subject)
			.resource(p.resource)
			.issuer(p.issuer)
			.client(reinhardt::db::orm::Json::new(p.client))
			.scopes(p.scopes)
			.pkce_verifier(p.pkce_verifier)
			.redirect_uri(p.redirect_uri)
			.return_to(p.return_to)
			.connection(p.connection)
			.revision(p.revision)
			.expires_at(
				chrono::Utc::now()
					+ chrono::Duration::minutes(
						aidash_application::oauth_connections::LIFETIME_MINUTES,
					),
			)
			.finish()
			.reserve(&f.store.control_pool)
			.await?;
		let mut response = Response::ok().with_json(&Started {
			authorization_url: request.authorization_url,
		})?;
		oidc::set_cookie(
			&mut response,
			COOKIE,
			&browser,
			config,
			true,
			aidash_application::oauth_connections::LIFETIME_MINUTES * 60,
		)?;
		oidc::no_store(&mut response);
		Ok(response)
	}
	pub(crate) async fn callback(
		&self,
		headers: HeaderMap,
		query: CallbackQuery,
	) -> Result<Response> {
		let f = &self.runtime;
		let config = oidc::required_config(f)?;
		let browser = oidc::cookie_value(&headers, &oidc::cookie_name(COOKIE, config))
			.ok_or(Error::Unauthorized)?;
		let state = query
			.state
			.filter(|s| !s.is_empty() && s.len() <= 512)
			.ok_or(Error::Unauthorized)?;
		let pending = OAuthAuthorization::consume(&f.store.control_pool, digest(&state)).await?;
		if pending.browser_hash != digest(browser)
			|| pending.redirect_uri != format!("{}/auth/connections/callback", config.public_origin)
		{
			return Err(Error::Unauthorized);
		}
		let session = oidc::browser_session_from_headers(f, &headers).await?;
		if session.id != pending.session_id {
			return Err(Error::Unauthorized);
		}
		let mut scoped = headers;
		scoped.insert(
			"x-aidash-context",
			pending.context.parse().map_err(|_| Error::Unauthorized)?,
		);
		let (actor, origin) = oidc::actor_from_headers(f, &scoped, &Method::GET).await?;
		let current = owner(&actor, Some(&origin), &pending.tenant)?;
		let (_, access) = self.personal_owner(&actor, &pending.tenant).await?;
		access.finish(Ok(())).await?;
		if origin.session.id != pending.session_id || current.subject != pending.subject {
			return Err(Error::Unauthorized);
		}
		let return_to = pending.return_to.clone();
		let outcome = match self
			.service()?
			.complete(
				pending.pending(),
				query.code.as_deref(),
				query.error.as_deref(),
			)
			.await
		{
			Ok(outcome) => outcome,
			Err(_) => {
				tracing::warn!("MCP OAuth authorization failed");
				Outcome::Failed
			}
		};
		popup_response(config.public_origin, &return_to, outcome)
	}
}
/// The nonce is generated for this response only. Neither provider input nor
/// callback state is interpolated into the executable page.
fn popup_response(origin: &str, return_to: &str, outcome: Outcome) -> Result<Response> {
	let nonce = oidc::random_secret();
	let separator = if return_to
		.split('#')
		.next()
		.unwrap_or_default()
		.contains('?')
	{
		'&'
	} else {
		'?'
	};
	let (path, fragment) = return_to
		.split_once('#')
		.map_or((return_to, None), |(p, f)| (p, Some(f)));
	let mut location = format!("{path}{separator}connection={}", outcome.as_str());
	if let Some(f) = fragment {
		location.push('#');
		location.push_str(f);
	}
	// Escape HTML's script terminator even in a validated same-origin return path.
	let json = |s: &str| {
		serde_json::to_string(s)
			.map(|s| s.replace('<', "\\u003c"))
			.map_err(Error::from)
	};
	let escaped = location
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
		.replace('\'', "&#39;");
	let html = format!(
		"<!doctype html><meta charset=utf-8><title>Connection</title><script nonce=\"{nonce}\">if(window.opener){{window.opener.postMessage({{type:'aidash.connection',outcome:{}}},{});window.close();}}else{{window.location.replace({});}}</script><noscript><a href=\"{escaped}\">Return to Aidash</a></noscript>",
		json(outcome.as_str())?,
		json(origin)?,
		json(&location)?
	);
	let mut response=Response::new(StatusCode::OK).with_body(html).with_header("Content-Type","text/html; charset=utf-8").with_header("Content-Security-Policy",&format!("default-src 'none'; script-src 'nonce-{nonce}'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'"));
	oidc::no_store(&mut response);
	Ok(response)
}
/// Retires grants only after the subject has no enabled Mapping left.
pub(crate) async fn retire_unmapped(
	f: &Federation,
	tenant: &str,
	subject: &str,
	_actor: &str,
) -> Result<()> {
	use super::super::models::DashboardMapping;
	use reinhardt::db::orm::Model;
	let Some(service) = &f.store.oauth_connections else {
		return Ok(());
	};
	let mapped = f
		.store
		.control_pool
		.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
			Ok(DashboardMapping::objects()
				.filter(DashboardMapping::field_tenant().eq(tenant))
				.filter(DashboardMapping::field_subject().eq(subject))
				.filter(DashboardMapping::field_enabled().eq(true))
				.all_with_db(tx)
				.await?
				.len())
		})
		.await?;
	if mapped == 0 {
		service.retire_subject(tenant, subject).await?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn oauth_connections_return_path_stays_on_the_dashboard_origin() {
		let origin = reqwest::Url::parse("https://dashboard.example").unwrap();
		for path in [
			"https://other.example",
			"//other.example",
			"/\\other.example",
			"/\t/other.example",
			"/\n/other.example",
			"/\r/other.example",
		] {
			assert!(dashboard_path(Some(path)).is_err(), "{path:?}");
		}
		let path = dashboard_path(Some("/settings?q=connections#person")).unwrap();
		assert_eq!(origin.join(path).unwrap().origin(), origin.origin());
	}
	#[test]
	fn oauth_connections_unsupported_store_returns_a_retryable_http_failure() {
		let response =
			Error::from(aidash_application::Error::ConnectionStoreUnavailable).http_response();
		assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
		assert_eq!(
			serde_json::from_slice::<serde_json::Value>(&response.body).unwrap(),
			serde_json::json!({"error":"connection_unavailable"})
		);
	}
	#[test]
	fn callback_document_binds_its_script_to_csp_and_escapes_return_destinations() {
		let response = popup_response(
			"https://dashboard.example",
			"/settings?next=</script><script>alert(1)</script>&q=\"#connections",
			Outcome::Connected,
		)
		.unwrap();
		let html = std::str::from_utf8(&response.body).unwrap();
		let policy = response.headers["content-security-policy"]
			.to_str()
			.unwrap();
		let nonce = policy
			.split("'nonce-")
			.nth(1)
			.unwrap()
			.split('\'')
			.next()
			.unwrap();
		assert!(html.contains(&format!("nonce=\"{nonce}\"")));
		assert_eq!(html.matches("</script>").count(), 1);
		assert!(html.contains("\\u003c/script>"));
		assert!(html.contains("\"https://dashboard.example\""));
		assert!(html.contains("outcome:\"connected\""));
		assert!(html.contains("&amp;q=&quot;&amp;connection=connected#connections"));
		assert_eq!(response.headers["cache-control"], "no-store");
	}
}
