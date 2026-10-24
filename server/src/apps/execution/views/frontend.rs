//! Browser routes and bundle assets served by the native HTTP stack.
use super::super::services::frontend::Frontend;
use async_trait::async_trait;
use reinhardt::core::endpoint::EndpointInfo;
use reinhardt::http::{Handler, ViewResult};
use reinhardt::{Depends, Method, Path, Request, Response, get};

#[get("/", name = "frontend-index")]
pub async fn index(
	#[inject] frontend: Depends<Frontend>,
	request: Request,
) -> ViewResult<Response> {
	Ok(frontend
		.response(&request, "", request.method == Method::HEAD)
		.await)
}

pub fn index_head() -> impl Handler + EndpointInfo {
	Head(index())
}

#[get("/{<path:asset>}", name = "frontend-asset")]
pub async fn asset(
	#[inject] frontend: Depends<Frontend>,
	Path(asset): Path<String>,
	request: Request,
) -> ViewResult<Response> {
	Ok(frontend
		.response(&request, &asset, request.method == Method::HEAD)
		.await)
}

pub fn asset_head() -> impl Handler + EndpointInfo {
	Head(asset())
}

// Reuse the GET endpoint's native extractors and DI for HEAD requests.
struct Head<E>(E);

impl<E: EndpointInfo> EndpointInfo for Head<E> {
	fn path() -> &'static str {
		E::path()
	}
	fn method() -> Method {
		Method::HEAD
	}
	fn name() -> &'static str {
		if E::path() == "/" {
			"frontend-index-head"
		} else {
			"frontend-asset-head"
		}
	}
}

#[async_trait]
impl<E: Handler> Handler for Head<E> {
	async fn handle(&self, request: Request) -> ViewResult<Response> {
		self.0.handle(request).await
	}
}
