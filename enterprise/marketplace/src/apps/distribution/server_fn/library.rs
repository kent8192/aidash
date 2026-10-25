//! Typed full-stack Pages boundary. Only serializable data crosses the wire.
use super::super::serializers::*;
#[cfg(server)]
use super::super::server::rpc::Browser;
#[cfg(server)]
use reinhardt::pages::server_fn::ServerFnRequest;
use reinhardt::pages::server_fn::{ServerFnError, server_fn};

#[server_fn]
pub async fn marketplace_library(
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Library, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::GET,
			"/api/marketplace/library",
			None,
			None,
		)
		.await
}

#[server_fn]
pub async fn marketplace_acquire(
	plugin_id: String,
	csrf: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Acquisition, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			&(format!(
				"/api/marketplace/plugins/{}/acquire",
				uuid::Uuid::parse_str(&plugin_id)
					.map_err(|_| ServerFnError::server(400, "invalid plugin"))?
			)),
			Some(serde_json::json!({})),
			Some(&csrf),
		)
		.await
}

#[server_fn]
pub async fn marketplace_logout(
	csrf: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Session, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/auth/logout",
			Some(serde_json::json!({})),
			Some(&csrf),
		)
		.await
}
