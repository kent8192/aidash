//! Request authorization helpers.
use crate::{
	authorization::{identity::Actor, workspace::Workspaces},
	federation::Federation,
};
use http::{HeaderMap, Method};

/// The actual authenticated HTTP request binding, including its original query.
/// Both admission and the later effect owner read the same request bytes.
pub(crate) fn peer_request(
	request: &reinhardt::Request,
) -> Option<aidash_application::federation::inbound::RequestDigest<'_>> {
	Some(aidash_application::federation::inbound::RequestDigest {
		method: request.method.as_str(),
		path: request
			.uri
			.path_and_query()?
			.as_str()
			.strip_prefix("/federation/v0.1")?,
		body: request.body(),
	})
}

/// The verified Federation caller of a request, set by the peer boundary and
/// injected into the routes that serve other Nodes.
#[derive(Clone)]
pub(crate) struct InboundCall(pub aidash_application::federation::inbound::Admitted);

#[async_trait::async_trait]
impl reinhardt::Injectable for InboundCall {
	async fn inject(ctx: &reinhardt::InjectionContext) -> reinhardt::DiResult<Self> {
		ctx.get_http_request()
			.and_then(|request| request.extensions.get::<Self>())
			.ok_or_else(|| reinhardt::DiError::NotFound("admitted Federation caller".into()))
	}
}
pub(crate) fn scoped(f: &Federation, actor: Actor) -> Option<Workspaces> {
	match actor {
		Actor::DeploymentAdministrator => None,
		Actor::Subject(identity) => Some(Workspaces {
			store: f.store.clone(),
			identity,
		}),
	}
}
pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
	let (scheme, token) = headers
		.get("authorization")?
		.to_str()
		.ok()?
		.split_once(' ')?;
	scheme.eq_ignore_ascii_case("Bearer").then_some(token)
}
/// The writes a browser Deployment Administrator may send. They are exactly the
/// writes on routes the boundary admits administrators to (`administered()` and
/// `deployment_administrator()`); Tenant content is refused by the boundary
/// itself, so nothing here grants Tenant authority (#484, #301).
pub(crate) fn browser_deployment_administrator_allowed(method: &Method, path: &str) -> bool {
	if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
		return true;
	}
	let path = path.strip_prefix("/api").unwrap_or(path);
	let segments: Vec<&str> = path.split('/').collect();
	// Browser Deployment Administrator mode administers the installation and may stop work.
	// It cannot silently become a tenant subject for new work or resumption.
	if matches!(segments.as_slice(), ["", "runs", _, "management"]) {
		return true;
	}
	// Deployment Administrators issue and revoke Invitations for any Tenant.
	if *method == Method::POST
		&& matches!(
			segments.as_slice(),
			["", "tenants", _, "invitations"] | ["", "tenants", _, "invitations", _, "revoke"]
		) {
		return true;
	}
	// Deployment Administrators purge a deleted Node before its grace period
	// ends (ADR 0083); every other Node change is Tenant administration.
	if *method == Method::POST
		&& matches!(segments.as_slice(), ["", "tenants", _, "nodes", _, "purge"])
	{
		return true;
	}
	path.starts_with("/dashboard/") || path.starts_with("/authorization/")
}

#[cfg(test)]
#[path = "../tests/http_auth.rs"]
mod tests;
