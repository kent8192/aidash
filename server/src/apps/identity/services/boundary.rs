//! Authentication and transaction visibility at the HTTP boundary.
use super::http_auth::{bearer, browser_operator_allowed, peer_node};
use crate::{
	Error, Result,
	authorization::{Authorization, identity::Actor},
	config::{PROTOCOL_VERSION, same_secret},
	dashboard_auth,
	federation::Federation,
	transactions::gate::ReadLease,
};
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
	Operator,
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
	pub fn authenticated() -> Self {
		Self {
			requirement: Requirement::Authenticated,
			visibility: false,
		}
	}
	pub fn operator() -> Self {
		Self {
			requirement: Requirement::Operator,
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
				if request
					.headers
					.get("x-aidash-protocol")
					.and_then(|v| v.to_str().ok())
					!= Some(PROTOCOL_VERSION)
				{
					return Err(Error::Invalid("unsupported federation protocol".into()));
				}
				runtime
					.authenticate_peer(
						peer_node(&request.headers)?,
						bearer(&request.headers).ok_or(Error::Unauthorized)?,
					)
					.await?;
				request.extensions.insert(crate::http::AuthenticatedPeer(
					peer_node(&request.headers)?.to_owned(),
				));
				return Ok(());
			}
			Requirement::Authenticated | Requirement::Operator => {}
		}
		let token = bearer(&request.headers);
		let actor = if token.is_some_and(|token| same_secret(token, &runtime.config.api_token)) {
			Actor::Operator
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
			if matches!(actor, Actor::Operator)
				&& !browser_operator_allowed(&request.method, request.uri.path())
			{
				return Err(Error::Forbidden);
			}
			request.extensions.insert(origin);
			actor
		};
		if matches!(self.requirement, Requirement::Operator) && !matches!(actor, Actor::Operator) {
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
						path.strip_suffix("/operator-grant")
							.or_else(|| path.strip_suffix("/restore"))
							.or_else(|| path.strip_suffix("/display-erasure"))
					})
					.and_then(|id| uuid::Uuid::parse_str(id).ok())
					== Some(origin.identity_id)
			});
			if self_identity_write && request.method != Method::GET {
				return Err(Error::Forbidden);
			}
			let mut operator_lease = if matches!(actor, Some(Actor::Operator))
				&& request.method != Method::GET
				&& request.method != Method::HEAD
				&& request.method != Method::OPTIONS
				&& let Some(origin) = browser
			{
				let mut lease = crate::database::native::begin(&runtime.store.pool).await?;
				origin.require_operator(&mut lease, true).await?;
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
			if let Some(lease) = operator_lease.take() {
				lease.commit().await?;
			}
			if matches!(
				self.requirement,
				Requirement::Authenticated | Requirement::Operator
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
