//! HTTP and MCP transports for validated application tools.
use aidash_application::execution::bounded_utf8_end;
use aidash_application::{
	Error, Result,
	ports::{Credentials, tools::ToolTransport},
	tools::required,
};
use aidash_domain::tool::ToolConfig;
use async_trait::async_trait;
use rmcp::{
	ServiceExt,
	model::CallToolRequestParams,
	transport::{
		StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
	},
};
use serde_json::{Value, json};
use std::sync::Arc;
mod mcp;

pub struct HttpTools {
	pub client: reqwest::Client,
	pub credentials: Arc<dyn Credentials>,
}
#[async_trait]
impl ToolTransport for HttpTools {
	async fn invoke(&self, configuration: &ToolConfig, input: Value, key: &str) -> Result<Value> {
		match configuration {
			ToolConfig::Native { operation, .. } if operation == "http_get" => {
				let url = reqwest::Url::parse(required(&input, "url")?)
					.map_err(|_| Error::Invalid("invalid URL".into()))?;
				let response = self
					.client
					.get(url.clone())
					.send()
					.await
					.map_err(crate::http_error)?;
				web_fetch_response(response, &url).await
			}
			ToolConfig::Http {
				endpoint,
				credential_env,
				..
			} => {
				let mut req = self
					.client
					.post(endpoint)
					.header("idempotency-key", key)
					.json(&input);
				if let Some(name) = credential_env {
					req = req.bearer_auth(self.credentials.resolve(name)?);
				}
				let response = req.send().await.map_err(crate::http_error)?;
				if !response.status().is_success() {
					return Err(Error::External(format!(
						"HTTP tool returned {}",
						response.status()
					)));
				}
				crate::response::json(response, 256_000).await
			}
			ToolConfig::Mcp {
				endpoint,
				credential_env,
				tool_name,
				idempotency_argument,
				..
			} => {
				let mut transport_config =
					StreamableHttpClientTransportConfig::with_uri(endpoint.clone());
				if let Some(name) = credential_env {
					transport_config.auth_header = Some(self.credentials.resolve(name)?);
				}
				let transport = StreamableHttpClientTransport::with_client(
					mcp::BoundedClient(self.client.clone()),
					transport_config,
				);
				let service = ()
					.serve(transport)
					.await
					.map_err(|e| Error::External(format!("MCP initialization failed: {e}")))?;
				let mut args = input
					.as_object()
					.cloned()
					.ok_or_else(|| Error::Invalid("tool arguments must be an object".into()))?;
				if let Some(argument) = idempotency_argument {
					args.insert(argument.clone(), json!(key));
				}
				let result = service
					.call_tool(CallToolRequestParams::new(tool_name.clone()).with_arguments(args))
					.await
					.map_err(|e| Error::External(format!("MCP call failed: {e}")));
				let _ = service.cancel().await;
				let result = result?;
				if result.is_error == Some(true) {
					return Ok(json!({"is_error":true,"content":result.content}));
				}
				Ok(serde_json::to_value(result)?)
			}
			_ => Err(Error::Invalid(
				"tool configuration has no external transport".into(),
			)),
		}
	}
}
async fn web_fetch_response(mut response: reqwest::Response, url: &reqwest::Url) -> Result<Value> {
	let status = response.status();
	if matches!(
		status,
		reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND
	) {
		// These terminal source rejections are evidence the agent can work around.
		// Other errors, including transient 408/429 responses, retain worker retries.
		let url = &url.as_str()[..bounded_utf8_end(url.as_str(), 0, 2048)];
		return Ok(
			json!({"ok":false,"error":{"kind":"http_status","url":url,"status":status.as_u16()}}),
		);
	}
	response.error_for_status_ref().map_err(crate::http_error)?;
	let mut bytes = Vec::new();
	while let Some(chunk) = response.chunk().await.map_err(crate::http_error)? {
		if bytes.len() + chunk.len() > 256_000 {
			return Err(Error::Invalid("tool response exceeds 256 KB".into()));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(json!({"text":String::from_utf8_lossy(&bytes)}))
}

#[cfg(test)]
mod tests;
