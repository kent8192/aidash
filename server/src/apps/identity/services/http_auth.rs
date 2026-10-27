//! Request authorization helpers.
use crate::{
	authorization::{identity::Actor, workspace::Workspaces},
	federation::Federation,
};
use http::{HeaderMap, Method};

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
	if matches!(
		segments.as_slice(),
		["", "runs", _, "control" | "management"]
	) {
		return true;
	}
	// Invitations, Registrations, Mappings, Tenant Deletion and Tenant labels
	// are administered per Tenant.
	if *method == Method::POST
		&& matches!(
			segments.as_slice(),
			["", "tenants", _, "invitations"]
				| ["", "tenants", _, "invitations", _, "revoke"]
				| ["", "tenants", _, "registrations", _, "approve" | "reject"]
				| ["", "tenants", _, "mappings", _, "disable"]
				| ["", "tenants", _, "deletion" | "purge"]
		) {
		return true;
	}
	if *method == Method::PUT && matches!(segments.as_slice(), ["", "tenants", _, "label"]) {
		return true;
	}
	path.starts_with("/dashboard/") || path.starts_with("/authorization/")
}

#[cfg(test)]
#[path = "../tests/http_auth.rs"]
mod tests;

/// The viewer a Peer Graph Grant admitted: a Deployment Administrator of
/// another Deployment, which has no Node.
#[derive(Clone)]
pub(crate) struct ViewerCall(pub aidash_application::federation::inbound::AdmittedViewer);

/// Who called the one route that serves both a Node and a grant's viewer.
#[derive(Clone)]
pub(crate) enum GraphCaller {
	Node(aidash_application::federation::inbound::Admitted),
	Viewer(aidash_application::federation::inbound::AdmittedViewer),
}

#[async_trait::async_trait]
impl reinhardt::Injectable for GraphCaller {
	async fn inject(ctx: &reinhardt::InjectionContext) -> reinhardt::DiResult<Self> {
		let request = ctx
			.get_http_request()
			.ok_or_else(|| reinhardt::DiError::NotFound("admitted Federation caller".into()))?;
		if let Some(call) = request.extensions.get::<InboundCall>() {
			return Ok(Self::Node(call.0));
		}
		request
			.extensions
			.get::<ViewerCall>()
			.map(|call| Self::Viewer(call.0))
			.ok_or_else(|| reinhardt::DiError::NotFound("admitted Federation caller".into()))
	}
}
