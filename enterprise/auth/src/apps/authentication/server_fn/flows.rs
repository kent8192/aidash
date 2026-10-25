//! Typed full-stack Pages boundary. Only serializable data crosses the wire.
use super::super::serializers::*;
#[cfg(server)]
use super::super::server::rpc::Browser;
#[cfg(server)]
use reinhardt::pages::server_fn::ServerFnRequest;
use reinhardt::pages::server_fn::{ServerFnError, server_fn};

#[server_fn]
pub async fn auth_login_context(
	flow: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<LoginContext, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::GET,
			&(format!(
				"/login/context?flow={}",
				url::form_urlencoded::byte_serialize(flow.as_bytes()).collect::<String>()
			)),
			None,
		)
		.await
}

#[server_fn]
pub async fn auth_select_tenant(
	flow: String,
	csrf: String,
	tenant_id: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<LoginContext, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/login/tenant",
			Some(serde_json::json!({"flow":flow,"csrf":csrf,"tenant_id":tenant_id})),
		)
		.await
}

#[server_fn]
pub async fn auth_verify(
	flow: String,
	csrf: String,
	id_token: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Next, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/login/verify",
			Some(serde_json::json!({"flow":flow,"csrf":csrf,"id_token":id_token})),
		)
		.await
}

#[server_fn]
pub async fn auth_choose(
	flow: String,
	csrf: String,
	choice: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Next, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/login/choice",
			Some(serde_json::json!({"flow":flow,"csrf":csrf,"choice":choice})),
		)
		.await
}
