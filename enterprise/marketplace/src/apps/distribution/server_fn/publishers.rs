//! Typed full-stack Pages boundary. Only serializable data crosses the wire.
use super::super::serializers::*;
#[cfg(server)]
use super::super::server::rpc::Browser;
#[cfg(server)]
use reinhardt::pages::server_fn::ServerFnRequest;
use reinhardt::pages::server_fn::{ServerFnError, server_fn};

#[server_fn]
pub async fn marketplace_publishers(
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Publishers, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::GET,
			"/api/marketplace/publishers",
			None,
			None,
		)
		.await
}

#[server_fn]
pub async fn marketplace_publisher_plugins(
	publisher_id: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<Catalog, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::GET,
			&(format!(
				"/api/marketplace/publishers/{}/plugins",
				uuid::Uuid::parse_str(&publisher_id)
					.map_err(|_| ServerFnError::server(400, "invalid Publisher"))?
			)),
			None,
			None,
		)
		.await
}

#[server_fn]
pub async fn marketplace_create_publisher(
	name: String,
	csrf: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<CreatedPublisher, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/api/marketplace/publishers",
			Some(serde_json::json!({"name":name})),
			Some(&csrf),
		)
		.await
}

#[server_fn]
pub async fn marketplace_create_plugin(
	draft: PluginDraft,
	csrf: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<CreatedPlugin, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			"/api/marketplace/plugins",
			Some(
				serde_json::to_value(draft)
					.map_err(|_| ServerFnError::server(400, "invalid plugin"))?,
			),
			Some(&csrf),
		)
		.await
}

#[server_fn]
pub async fn marketplace_publish(
	plugin_id: String,
	publication: Publication,
	csrf: String,
	#[inject] runtime: Browser,
	#[inject] request: ServerFnRequest,
) -> Result<PublishedVersion, ServerFnError> {
	runtime
		.call(
			&request,
			http::Method::POST,
			&(format!(
				"/api/marketplace/plugins/{}/versions",
				uuid::Uuid::parse_str(&plugin_id)
					.map_err(|_| ServerFnError::server(400, "invalid plugin"))?
			)),
			Some(
				serde_json::to_value(publication)
					.map_err(|_| ServerFnError::server(400, "invalid publication"))?,
			),
			Some(&csrf),
		)
		.await
}
