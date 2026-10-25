//! Typed full-stack Pages boundary. Only serializable data crosses the wire.
use super::super::serializers::*;
#[cfg(server)]
use super::super::server::rpc::Browser;
#[cfg(server)]
use reinhardt::pages::server_fn::ServerFnRequest;
use reinhardt::pages::server_fn::{ServerFnError, server_fn};

#[server_fn]
pub async fn marketplace_connections(
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Connections, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::GET,
			"/api/marketplace/connections",
			None,
			None,
		)
		.await
}
