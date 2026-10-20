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
pub(crate) fn browser_deployment_administrator_allowed(method: &Method, path: &str) -> bool {
	if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
		return true;
	}
	let path = path.strip_prefix("/api").unwrap_or(path);
	if path.starts_with("/tenants/")
		&& (path.contains("/provider-credentials")
			|| path.contains("/provider-credential-bindings")
			|| path.contains("/provider-authorizations/")
			|| path.ends_with("/retention"))
	{
		return true;
	}
	// Deployment Administrators issue and revoke Invitations for any Tenant.
	if *method == Method::POST
		&& matches!(
			path.split('/').collect::<Vec<_>>().as_slice(),
			["", "tenants", _, "invitations"] | ["", "tenants", _, "invitations", _, "revoke"]
		) {
		return true;
	}
	let segments: Vec<&str> = path.split('/').collect();
	if matches!(
		segments.as_slice(),
		[
			"",
			"runs",
			_,
			"control" | "management" | "research-revocation"
		] | ["", "tasks", _, "abandon"]
	) {
		return true;
	}
	if *method == Method::POST
		&& matches!(
			segments.as_slice(),
			["", "agents", "personal"]
				| ["", "skills", "import"]
				| ["", "generation", _, "policies", _]
				| ["", "generation", _, "requests", _, "control"]
				| ["", "workspaces", _, "semantic", "index"]
				| ["", "workspaces", _, "semantic", "search"]
				| ["", "workspaces", _, "semantic", "entries"]
				| ["", "workspaces", _, "semantic", "entries", _, "reindex"]
				| ["", "remote"]
		) {
		return true;
	}
	if *method == Method::DELETE
		&& matches!(
			segments.as_slice(),
			["", "workspaces", _, "semantic", "entries", _]
		) {
		return true;
	}
	// Browser Deployment Administrator mode administers the installation and may stop work.
	// It cannot silently become a tenant subject for new work or resumption.
	path.starts_with("/dashboard/")
		|| path.starts_with("/authorization/")
		|| path.starts_with("/registry/")
		|| path.starts_with("/marketplace/")
		|| path.starts_with("/transactions/")
		|| path.starts_with("/workbench/")
		|| matches!(
			path,
			"/registry" | "/peers" | "/marketplace" | "/transactions"
		)
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
