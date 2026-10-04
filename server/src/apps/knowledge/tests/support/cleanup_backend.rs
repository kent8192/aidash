//! A native backend fixture that fails deletions until the test releases it.
use async_trait::async_trait;
use reinhardt::di::{DiError, DiResult, Injectable};
use reinhardt::http::ViewResult;
use reinhardt::test::fixtures::injection_context;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::{InjectionContext, Json, Path, Response, ServerRouter, StatusCode, delete, post};
use rstest::fixture;
use serde_json::{Value, json};
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default)]
pub struct CleanupState {
	pub available: Arc<AtomicBool>,
	pub requests: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Injectable for CleanupState {
	async fn inject(context: &InjectionContext) -> DiResult<Self> {
		context
			.get_singleton::<Self>()
			.map(|state| (*state).clone())
			.ok_or_else(|| DiError::NotFound("cleanup fixture state".into()))
	}
}

impl CleanupState {
	fn reply(&self, identity: String) -> ViewResult<Response> {
		self.requests.lock().unwrap().push(identity);
		let status = if self.available.load(Ordering::SeqCst) {
			// Deleting an already absent physical identity is acknowledged.
			StatusCode::NOT_FOUND
		} else {
			StatusCode::SERVICE_UNAVAILABLE
		};
		Response::new(status).with_json(&json!({"status":"fixture"}))
	}
}

#[post("/collections/{collection}/points/delete")]
async fn point(
	#[inject] state: CleanupState,
	Path(collection): Path<String>,
	Json(body): Json<Value>,
) -> ViewResult<Response> {
	state.reply(format!(
		"point:{collection}:{}",
		body["points"][0].as_str().unwrap()
	))
}

#[delete("/collections/{collection}")]
async fn collection(
	#[inject] state: CleanupState,
	Path(collection): Path<String>,
) -> ViewResult<Response> {
	state.reply(format!("collection:{collection}"))
}

pub struct CleanupBackend {
	pub state: CleanupState,
	pub server: TestServerGuard,
}

#[fixture]
pub async fn cleanup_backend(injection_context: InjectionContext) -> CleanupBackend {
	let state = CleanupState::default();
	injection_context.set_singleton(state.clone());
	let router = ServerRouter::new()
		.endpoint(point)
		.endpoint(collection)
		.with_di_context(Arc::new(injection_context));
	CleanupBackend {
		state,
		server: test_server_guard(router).await,
	}
}
