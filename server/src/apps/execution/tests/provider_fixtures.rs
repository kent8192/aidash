//! Native HTTP provider fixtures shared by adapter and execution regressions.
use async_trait::async_trait;
use reinhardt::di::{DiError, DiResult, Injectable};
use reinhardt::http::ViewResult;
use reinhardt::test::fixtures::injection_context;
use reinhardt::test::fixtures::server::{TestServerGuard, test_server_guard};
use reinhardt::{InjectionContext, Json, Response, ServerRouter, StatusCode, post};
use rstest::fixture;
use serde_json::{Value, json};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::{Notify, mpsc};

#[derive(Clone)]
pub enum Reply {
	Completion,
	Final,
	Unavailable,
	Delayed(Arc<Notify>),
}

#[derive(Clone)]
struct CompletionState {
	sender: mpsc::UnboundedSender<Value>,
	reply: Reply,
}

#[async_trait]
impl Injectable for CompletionState {
	async fn inject(context: &InjectionContext) -> DiResult<Self> {
		context
			.get_singleton::<Self>()
			.map(|state| (*state).clone())
			.ok_or_else(|| DiError::NotFound("provider fixture state".into()))
	}
}

#[post("/chat/completions")]
async fn completion(
	#[inject] state: CompletionState,
	Json(body): Json<Value>,
) -> ViewResult<Response> {
	state.sender.send(body.clone()).unwrap();
	let respond_to_tools = matches!(state.reply, Reply::Completion);
	match state.reply {
		Reply::Unavailable => {
			return Response::new(StatusCode::NOT_FOUND)
				.with_json(&json!({"error":"No ZDR endpoints"}));
		}
		Reply::Delayed(release) => release.notified().await,
		Reply::Completion | Reply::Final => {}
	}
	let choice = if respond_to_tools && body["tools"].is_array() {
		json!({"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{
			"id":"call-1","type":"function","function":{"name":"read","arguments":"{\"path\":\"notes\"}"}
		}]}})
	} else {
		json!({"finish_reason":"stop","message":{"content":"Completed"}})
	};
	Response::ok().with_json(&json!({
		"choices":[choice],"usage":{"prompt_tokens":12,"completion_tokens":7,"cost":0.01}
	}))
}

pub struct CompletionFixture {
	pub received: mpsc::UnboundedReceiver<Value>,
	pub server: TestServerGuard,
	release: Option<Arc<Notify>>,
}

#[fixture]
pub async fn completion_server(injection_context: InjectionContext) -> CompletionFixture {
	start(injection_context, Reply::Completion).await
}

#[fixture]
pub async fn final_server(injection_context: InjectionContext) -> CompletionFixture {
	start(injection_context, Reply::Final).await
}

#[fixture]
pub async fn unavailable_server(injection_context: InjectionContext) -> CompletionFixture {
	start(injection_context, Reply::Unavailable).await
}

#[fixture]
pub async fn delayed_server(injection_context: InjectionContext) -> CompletionFixture {
	start(injection_context, Reply::Delayed(Arc::new(Notify::new()))).await
}

async fn start(context: InjectionContext, reply: Reply) -> CompletionFixture {
	let (sender, received) = mpsc::unbounded_channel();
	let release = match &reply {
		Reply::Delayed(release) => Some(release.clone()),
		_ => None,
	};
	context.set_singleton(CompletionState { sender, reply });
	let router = ServerRouter::new()
		.endpoint(completion)
		.mount("/api/v1/", ServerRouter::new().endpoint(completion))
		.mount("/v1/", ServerRouter::new().endpoint(completion))
		.with_di_context(Arc::new(context));
	CompletionFixture {
		received,
		server: test_server_guard(router).await,
		release,
	}
}

impl CompletionFixture {
	pub async fn respond_after<T>(
		&mut self,
		request: impl Future<Output = T>,
		delay_secs: u64,
	) -> (T, Duration) {
		tokio::pin!(request);
		// Establish real network I/O before pausing Tokio's virtual clock.
		tokio::time::timeout(Duration::from_secs(5), async {
			tokio::select! {
				_ = &mut request => panic!("request completed before the server received it"),
				body = self.received.recv() => {
					let body = body.expect("server must receive the request");
					assert!(body.get("request_timeout_secs").is_none());
				}
			}
		})
		.await
		.expect("local request must reach the server");

		tokio::time::pause();
		let started = tokio::time::Instant::now();
		tokio::select! {
			result = &mut request => {
				let elapsed = started.elapsed();
				tokio::time::resume();
				(result, elapsed)
			}
			() = tokio::time::sleep(Duration::from_secs(delay_secs)) => {
				let elapsed = started.elapsed();
				// Resume before releasing the response to avoid advancing through
				// a request deadline while network I/O is still in transit.
				tokio::time::resume();
				self.release.as_ref().expect("delayed fixture").notify_one();
				let result = tokio::time::timeout(Duration::from_secs(5), &mut request)
					.await
					.expect("released response must complete");
				(result, elapsed)
			}
		}
	}
}

impl Drop for CompletionFixture {
	fn drop(&mut self) {
		// Release a cancelled inference before dropping the fixture listener.
		if let Some(release) = &self.release {
			release.notify_one();
		}
	}
}
