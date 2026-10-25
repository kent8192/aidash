//! Typed full-stack Pages boundary. Only serializable data crosses the wire.
use super::super::serializers::*;
#[cfg(server)]
use super::super::server::rpc::Browser;
#[cfg(server)]
use reinhardt::pages::server_fn::ServerFnRequest;
use reinhardt::pages::server_fn::{ServerFnError, server_fn};

#[server_fn]
pub async fn auth_account(
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Account, ServerFnError> {
	runtime
		.call(&request, http::Method::GET, "/account", None)
		.await
}

#[server_fn]
pub async fn auth_link(
	csrf: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Next, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/account/link/start",
			Some(serde_json::json!({"csrf":csrf})),
		)
		.await
}

#[server_fn]
pub async fn auth_unlink(
	csrf: String,
	identity_id: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Next, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/account/unlink/start",
			Some(serde_json::json!({"csrf":csrf,"identity_id":identity_id})),
		)
		.await
}

#[server_fn]
pub async fn auth_logout_all(
	csrf: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<SignOut, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/logout/all",
			Some(serde_json::json!({"csrf":csrf})),
		)
		.await
}
