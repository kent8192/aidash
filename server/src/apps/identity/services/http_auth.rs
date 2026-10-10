//! Request authorization helpers.
use crate::{
	Error, Result,
	authorization::{identity::Actor, workspace::Workspaces},
	federation::Federation,
};
use http::{HeaderMap, Method};

pub(crate) fn peer_node(headers: &HeaderMap) -> Result<&str> {
	headers
		.get("x-aidash-node")
		.and_then(|h| h.to_str().ok())
		.ok_or(Error::Unauthorized)
}
pub(crate) fn scoped(f: &Federation, actor: Actor) -> Option<Workspaces> {
	match actor {
		Actor::Operator => None,
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
pub(crate) fn browser_operator_allowed(method: &Method, path: &str) -> bool {
	if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
		return true;
	}
	let path = path.strip_prefix("/api").unwrap_or(path);
	if path.starts_with("/tenants/")
		&& (path.contains("/provider-credentials")
			|| path.contains("/provider-credential-bindings"))
	{
		return true;
	}
	let segments: Vec<&str> = path.split('/').collect();
	if matches!(
		segments.as_slice(),
		["", "runs", _, "control" | "management"] | ["", "tasks", _, "abandon"]
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
	// Browser operator mode administers the installation and may stop work.
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
