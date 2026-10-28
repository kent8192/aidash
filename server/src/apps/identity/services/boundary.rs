//! Authentication and transaction visibility at the HTTP boundary.
use super::http_auth::{InboundCall, bearer, browser_deployment_administrator_allowed};
use crate::apps::identity::models::DashboardRegistrationRequest;
use crate::{
	Error, Result,
	authorization::{Authorization, identity::Actor},
	config::{PROTOCOL_VERSION, same_secret},
	dashboard_auth,
	federation::Federation,
	transactions::gate::ReadLease,
};
use aidash_application::{
	federation::inbound::{Inbound, Proof, RequestDigest, accept},
	tenancy::TenancyScope,
};
use aidash_capability::federation::FederationClaims;
use aidash_domain::federation::protocol::compatible;
use aidash_domain::nodes::NodeAddress;
use async_trait::async_trait;
use http::Method;
use http::header::{AUTHORIZATION, CACHE_CONTROL};
use reinhardt::http::{Handler, Middleware, ViewResult};
use reinhardt::{Injectable, InjectionContext, Request, Response};
use std::sync::Arc;

#[derive(Clone, Copy)]
enum Requirement {
	Public,
	Authenticated,
	Administered,
	DeploymentAdministrator,
	Peer,
}

/// Route policy evaluated before resolving request-scoped services.
#[derive(Clone, Copy)]
pub struct AccessBoundary {
	requirement: Requirement,
	visibility: bool,
}

impl AccessBoundary {
	pub fn public() -> Self {
		Self {
			requirement: Requirement::Public,
			visibility: false,
		}
	}
	/// A Tenant route: only a Tenant Subject (browser Mapping or Subject
	/// Credential) is admitted. A Deployment Administrator, whether a browser
	/// grant context or the shared `api_token` bearer, is refused with 403
	/// before the handler runs, so Tenant content never reaches it (#484, #301).
	pub fn authenticated() -> Self {
		Self {
			requirement: Requirement::Authenticated,
			visibility: false,
		}
	}
	/// A route that is part of administering the Deployment: both Tenant
	/// Subjects and Deployment Administrators are admitted, and the handler
	/// keeps its own per-actor checks. Use it only where an administrator is
	/// meant to act, never for Tenant content (#484, #301).
	pub fn administered() -> Self {
		Self {
			requirement: Requirement::Administered,
			visibility: false,
		}
	}
	pub fn deployment_administrator() -> Self {
		Self {
			requirement: Requirement::DeploymentAdministrator,
			visibility: false,
		}
	}
	pub fn peer() -> Self {
		Self {
			requirement: Requirement::Peer,
			visibility: false,
		}
	}
	pub fn with_visibility(mut self) -> Self {
		self.visibility = true;
		self
	}

	async fn authorize(&self, runtime: &Federation, request: &Request) -> Result<()> {
		match self.requirement {
			Requirement::Public => return Ok(()),
			Requirement::Peer => {
				if !request
					.headers
					.get("x-aidash-protocol")
					.and_then(|v| v.to_str().ok())
					.is_some_and(|peer| compatible(PROTOCOL_VERSION, peer))
				{
					return Err(Error::Invalid("unsupported federation protocol".into()));
				}
				admit_peer(runtime, request).await?;
				return Ok(());
			}
			Requirement::Authenticated
			| Requirement::Administered
			| Requirement::DeploymentAdministrator => {}
		}
		let token = bearer(&request.headers);
		let actor = if token.is_some_and(|token| same_secret(token, &runtime.config.api_token)) {
			Actor::DeploymentAdministrator
		} else if request.headers.contains_key(AUTHORIZATION)
			&& crate::apps::identity::repositories::desktop::access_token(&request.headers)
				.is_none()
		{
			Authorization {
				pool: runtime.store.pool.clone(),
			}
			.authenticate(token.ok_or(Error::Unauthorized)?)
			.await?
		} else {
			if runtime.config.dashboard_session().is_none() {
				return Err(Error::Unauthorized);
			}
			let (actor, origin) =
				dashboard_auth::actor_from_headers(runtime, &request.headers, &request.method)
					.await?;
			if matches!(actor, Actor::DeploymentAdministrator)
				&& !browser_deployment_administrator_allowed(&request.method, request.uri.path())
			{
				return Err(Error::Forbidden);
			}
			request.extensions.insert(origin);
			actor
		};
		if let Actor::Subject(identity) = &actor {
			// Even identity-only endpoints that do not open an Access transaction
			// are new protected operations. User-issued bearer credentials retain
			// their original source Mapping and must check the common authority.
			identity.check_binding(&runtime.store.pool).await?;
		}
		if matches!(self.requirement, Requirement::DeploymentAdministrator)
			&& !matches!(actor, Actor::DeploymentAdministrator)
		{
			return Err(Error::Forbidden);
		}
		if matches!(self.requirement, Requirement::Authenticated)
			&& matches!(actor, Actor::DeploymentAdministrator)
		{
			return Err(Error::Forbidden);
		}
		request.extensions.insert(actor);
		Ok(())
	}
}

#[async_trait]
impl Middleware for AccessBoundary {
	async fn process(&self, request: Request, next: Arc<dyn Handler>) -> ViewResult<Response> {
		let auth_response = request.uri.path().starts_with("/auth/");
		let result: Result<Response> = async {
			let context = request
				.get_di_context::<Arc<InjectionContext>>()
				.ok_or_else(|| Error::External("HTTP dependency context is unavailable".into()))?;
			let runtime = Federation::inject(&context)
				.await
				.map_err(|error| Error::External(error.to_string()))?;
			self.authorize(&runtime, &request).await?;
			let protection = reinhardt::Depends::<crate::http::Protection>::resolve_from_registry(
				&context, true,
			)
			.await
			.map_err(|e| Error::External(e.to_string()))?;
			let actor = request.extensions.get::<Actor>();
			if !protection.authenticated(&request, actor.as_ref()) {
				return Ok(Response::new(reinhardt::StatusCode::TOO_MANY_REQUESTS)
					.with_header("Retry-After", "1"));
			}
			let browser = request.extensions.get::<dashboard_auth::BrowserOrigin>();
			let self_identity_write = browser.as_ref().is_some_and(|origin| {
				request
					.uri
					.path()
					.strip_prefix("/api/dashboard/identities/")
					.and_then(|path| {
						path.strip_suffix("/deployment-administrator-grant")
							.or_else(|| path.strip_suffix("/restore"))
							.or_else(|| path.strip_suffix("/display-erasure"))
					})
					.and_then(|id| uuid::Uuid::parse_str(id).ok())
					== Some(origin.identity_id)
			});
			if self_identity_write && request.method != Method::GET {
				return Err(Error::Forbidden);
			}
			let registration_decision = request
				.uri
				.path()
				.strip_prefix("/api/dashboard/registrations/")
				.and_then(|path| {
					path.strip_suffix("/approve")
						.or_else(|| path.strip_suffix("/reject"))
				})
				.and_then(|id| uuid::Uuid::parse_str(id).ok());
			if let (Some(origin), Some(registration)) = (browser.as_ref(), registration_decision)
				&& request.method != Method::GET
			{
				// A decision locks the registration's identity for update in its own
				// transaction, while the Deployment Administrator lease below holds a shared lock on
				// the Deployment Administrator's identity: deciding one's own request could never finish.
				let identity = runtime
					.store
					.pool
					.atomic(&TenancyScope::NoTenant, async |tx| {
						DashboardRegistrationRequest::identity_of(tx, registration).await
					})
					.await?;
				if identity == Some(origin.identity_id) {
					return Err(Error::Forbidden);
				}
			}
			let mut deployment_administrator_lease =
				if matches!(actor, Some(Actor::DeploymentAdministrator))
					&& request.method != Method::GET
					&& request.method != Method::HEAD
					&& request.method != Method::OPTIONS
					&& let Some(origin) = browser
				{
					// Deployment Administrator grants are Deployment-wide.
					let mut lease = runtime.store.pool.begin(&TenancyScope::NoTenant).await?;
					origin
						.require_deployment_administrator(&mut lease, true)
						.await?;
					Some(lease)
				} else {
					None
				};
			let _visibility = if self.visibility {
				Some(ReadLease::begin(&runtime.store).await?)
			} else {
				None
			};
			let mut response = next.handle(request).await?;
			if let Some(lease) = deployment_administrator_lease.take() {
				lease.commit().await?;
			}
			if matches!(
				self.requirement,
				Requirement::Authenticated
					| Requirement::Administered
					| Requirement::DeploymentAdministrator
			) {
				response
					.headers
					.insert(CACHE_CONTROL, "no-store".parse().expect("static header"));
			}
			Ok(response)
		}
		.await;
		let mut response = result.unwrap_or_else(Error::http_response);
		if auth_response {
			super::oidc::no_store(&mut response);
		}
		Ok(response)
	}
}

/// Decides who a Federation request is, from the token it carries and the
/// request it is bound to (ADR 0064). Every refusal up to the signature is the
/// same `Unauthorized`.
async fn admit_peer(runtime: &Federation, request: &Request) -> Result<()> {
	let token = bearer(&request.headers).ok_or(Error::Unauthorized)?;
	let claims = FederationClaims::unverified(token).map_err(|_| Error::Unauthorized)?;
	let body = request.body();
	let path = request
		.uri
		.path()
		.strip_prefix("/federation/v0.1")
		.ok_or(Error::Unauthorized)?;
	let signed_path = match request.uri.query() {
		Some(query) => format!("{path}?{query}"),
		None => path.to_owned(),
	};
	let digest = RequestDigest {
		method: request.method.as_str(),
		path: &signed_path,
		body,
	};
	let (Ok(sender), Ok(receiver)) = (
		NodeAddress::parse(&claims.sub),
		NodeAddress::parse(&claims.aud),
	) else {
		return Err(Error::Unauthorized);
	};
	let admitted = accept(
		Inbound {
			receiver,
			sender: sender.clone(),
			subject: claims.subject,
			proof: Proof::Token(token.to_owned()),
		},
		&digest,
		runtime,
	)
	.await?;
	request
		.extensions
		.insert(crate::http::AuthenticatedPeer(sender.to_string()));
	request.extensions.insert(InboundCall(admitted));
	Ok(())
}
