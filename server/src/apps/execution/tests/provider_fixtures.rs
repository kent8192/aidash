//! Native HTTP provider fixtures shared by adapter and execution regressions.
use async_trait::async_trait;
use reinhardt::di::{DiError, DiResult, Injectable};
use reinhardt::http::ViewResult;
use reinhardt::test::fixtures::injection_context;
use reinhardt::test::fixtures::server::TestServerGuard;
#[path = "support/upstream.rs"]
mod upstream_fixtures;
use reinhardt::{InjectionContext, Json, Response, ServerRouter, StatusCode, post};
use rstest::fixture;
use serde_json::{Value, json};
use std::{
	future::Future,
	sync::{Arc, Mutex},
	time::Duration,
};
use tokio::sync::{Notify, mpsc};
use upstream_fixtures::upstream;

#[derive(Clone)]
#[allow(dead_code)] // Different integration binaries select different provider replies.
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

#[derive(Clone)]
struct CompletionChannel {
	sender: mpsc::UnboundedSender<Value>,
	received: Arc<Mutex<Option<mpsc::UnboundedReceiver<Value>>>>,
}

#[fixture]
fn completion_channel() -> CompletionChannel {
	let (sender, received) = mpsc::unbounded_channel();
	CompletionChannel {
		sender,
		received: Arc::new(Mutex::new(Some(received))),
	}
}

#[fixture]
fn completion_state(
	#[default(Reply::Completion)] reply: Reply,
	completion_channel: CompletionChannel,
) -> CompletionState {
	CompletionState {
		sender: completion_channel.sender,
		reply,
	}
}

#[fixture]
fn completion_router(
	completion_state: CompletionState,
	injection_context: InjectionContext,
) -> Arc<ServerRouter> {
	injection_context.set_singleton(completion_state);
	Arc::new(
		ServerRouter::new()
			.endpoint(completion)
			.mount("/api/v1/", ServerRouter::new().endpoint(completion))
			.mount("/v1/", ServerRouter::new().endpoint(completion))
			.with_di_context(Arc::new(injection_context)),
	)
}

#[fixture]
pub fn completion_server(
	#[default(Reply::Completion)] reply: Reply,
	completion_channel: CompletionChannel,
	#[from(completion_state)]
	#[with(reply.clone(), completion_channel.clone())]
	_state: CompletionState,
	#[from(completion_router)]
	#[with(_state.clone())]
	_router: Arc<ServerRouter>,
	#[future]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) -> impl Future<Output = CompletionFixture> {
	let server = Box::pin(server);
	async move {
		let release = match reply {
			Reply::Delayed(release) => Some(release),
			_ => None,
		};
		let received = completion_channel.received.lock().unwrap().take().unwrap();
		CompletionFixture {
			received,
			server: server.await,
			release,
		}
	}
}

#[fixture]
pub async fn final_server(
	#[future]
	#[from(completion_server)]
	#[with(Reply::Final)]
	server: CompletionFixture,
) -> CompletionFixture {
	server.await
}

#[fixture]
pub async fn unavailable_server(
	#[future]
	#[from(completion_server)]
	#[with(Reply::Unavailable)]
	server: CompletionFixture,
) -> CompletionFixture {
	server.await
}

#[fixture]
fn release() -> Arc<Notify> {
	Arc::new(Notify::new())
}

#[fixture]
pub async fn delayed_server(
	_release: Arc<Notify>,
	#[future]
	#[from(completion_server)]
	#[with(Reply::Delayed(_release.clone()))]
	server: CompletionFixture,
) -> CompletionFixture {
	server.await
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
