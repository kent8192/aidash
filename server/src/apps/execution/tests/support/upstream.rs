//! Owned native HTTP stubs for external-provider and peer contracts.
#![allow(dead_code)] // Integration targets consume different parts of this shared fixture.
use reinhardt::http::ViewResult;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::{Handler, Request, Response, ServerRouter};
use std::future::Future;

/// Keep listener and connection cleanup in Reinhardt's fixture guard.
pub async fn upstream_stub(router: ServerRouter) -> TestServerGuard {
	test_server_guard(router).await
}

/// Adapt capturing test closures to the framework's native Handler contract.
/// Method checks retain upstream route contracts without an auxiliary router.
pub fn handler<F, Fut>(method: http::Method, reply: F) -> impl Handler
where
	F: Fn(Request) -> Fut + Send + Sync + 'static,
	Fut: Future<Output = Response> + Send,
{
	StubHandler { method, reply }
}

struct StubHandler<F> {
	method: http::Method,
	reply: F,
}

#[async_trait::async_trait]
impl<F, Fut> Handler for StubHandler<F>
where
	F: Fn(Request) -> Fut + Send + Sync,
	Fut: Future<Output = Response> + Send,
{
	async fn handle(&self, request: Request) -> ViewResult<Response> {
		if request.method != self.method {
			return Ok(Response::new(http::StatusCode::METHOD_NOT_ALLOWED));
		}
		Ok((self.reply)(request).await)
	}
}
