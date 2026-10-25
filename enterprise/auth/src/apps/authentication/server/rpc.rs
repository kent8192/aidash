//! Native adapter shared by the typed Pages functions and protocol handlers.
//! Reuses the owning handler's cookie/CSRF/admission policies in process, never
//! a network hop or an arbitrary client-selected URL.
use http::{Method, header};
use reinhardt::di::{DiError, DiResult, Injectable, InjectionContext};
use reinhardt::pages::server_fn::{ServerFnError, ServerFnRequest};
use reinhardt::{Handler, Request};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;
#[derive(Clone)]
pub struct Browser(pub Arc<crate::service::Authentication>);
#[async_trait::async_trait]
impl Injectable for Browser {
	async fn inject(context: &InjectionContext) -> DiResult<Self> {
		context
			.get_singleton::<Self>()
			.map(|value| value.as_ref().clone())
			.ok_or_else(|| DiError::NotFound("auth service is not installed".into()))
	}
}
impl Browser {
	pub(crate) async fn call<T: DeserializeOwned>(
		&self,
		source: &ServerFnRequest,
		method: Method,
		path: &str,
		body: Option<Value>,
	) -> Result<T, ServerFnError> {
		let bytes = serde_json::to_vec(&body.unwrap_or(Value::Null))
			.map_err(|_| ServerFnError::server(500, "invalid request"))?;
		let mut request = Request::builder()
			.method(method)
			.uri(path)
			.body(bytes.into())
			.build()
			.map_err(|_| ServerFnError::server(400, "invalid request"))?;
		request.headers = source.inner().headers.clone();
		request.headers.insert(
			header::CONTENT_TYPE,
			header::HeaderValue::from_static("application/json"),
		);
		request.headers.remove(header::CONTENT_LENGTH);
		request.remote_addr = source.inner().remote_addr;
		request.is_secure = source.inner().is_secure;
		let response = crate::http::Http::new(self.0.clone())
			.handle(request)
			.await
			.map_err(|_| ServerFnError::server(503, "auth unavailable"))?;
		for cookie in response.headers.get_all(header::SET_COOKIE) {
			source.add_response_cookie(
				cookie
					.to_str()
					.map_err(|_| ServerFnError::server(500, "invalid session cookie"))?
					.to_owned(),
			);
		}
		if !response.status.is_success() {
			let message = serde_json::from_slice::<Value>(&response.body)
				.ok()
				.and_then(|value| {
					value
						.get("error")
						.and_then(Value::as_str)
						.map(str::to_owned)
				})
				.unwrap_or_else(|| "Request could not be completed".into());
			return Err(ServerFnError::server(response.status.as_u16(), message));
		}
		serde_json::from_slice(&response.body)
			.map_err(|_| ServerFnError::server(503, "invalid service response"))
	}
}
