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
		let actor = if request.headers.contains_key(AUTHORIZATION) {
			let token = bearer(&request.headers).ok_or(Error::Unauthorized)?;
			if same_secret(token, &runtime.config.api_token) {
				Actor::Operator
			} else {
				Authorization {
					pool: runtime.store.pool.clone(),
				}
				.authenticate(token)
				.await?
			}
		} else {
			if runtime.config.oidc.is_none() {
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
			let _visibility = if self.visibility {
				Some(ReadLease::begin(&runtime.store).await?)
			} else {
				None
			};
			let mut response = next.handle(request).await?;
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
		Ok(result.unwrap_or_else(Error::http_response))
	}
}
