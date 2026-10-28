//! Dedicated Keyless HTTP transport; no Aidash application routes or secrets.
use aidash_integrations::cli_backend::{
	Failure, Result,
	protocol::{MAX_INPUT, Request},
	provider::Backend,
};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited, StreamBody, combinators::UnsyncBoxBody};
use hyper::{
	Request as HttpRequest, Response, StatusCode,
	body::{Frame, Incoming},
	service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde_json::json;
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Semaphore, task::JoinSet};
type Body = UnsyncBoxBody<Bytes, Infallible>;

async fn infer(backend: &Backend, request: &Request) -> Result<serde_json::Value> {
	#[cfg(not(test))]
	{
		backend.infer(request).await
	}
	#[cfg(test)]
	{
		backend.fixture_infer(request).await
	}
}

fn response(status: StatusCode, value: serde_json::Value) -> Response<Body> {
	let mut response = Response::new(Full::new(Bytes::from(value.to_string())).boxed_unsync());
	*response.status_mut() = status;
	response
		.headers_mut()
		.insert("content-type", "application/json".parse().unwrap());
	response
		.headers_mut()
		.insert("cache-control", "no-store".parse().unwrap());
	response
}
fn error(status: StatusCode, code: &'static str) -> Response<Body> {
	response(status, json!({"error":{"code":code,"message":code}}))
}

async fn handle(
	req: HttpRequest<Incoming>,
	backend: Arc<Backend>,
	slots: Arc<Semaphore>,
) -> std::result::Result<Response<Body>, Infallible> {
	let origin = format!("127.0.0.1:{}", backend.config.port);
	if req.headers().contains_key("origin")
		|| req.headers().get("host").and_then(|v| v.to_str().ok()) != Some(origin.as_str())
	{
		return Ok(error(
			StatusCode::FORBIDDEN,
			"local_nonbrowser_client_required",
		));
	}
	if req.method() == "GET" && req.uri().path() == "/health" {
		return Ok(response(
			StatusCode::OK,
			json!({"status":"ok","inference_checked":false,"config":backend.config.diagnostic()}),
		));
	}
	if req.method() != "POST"
		|| req.uri().path() != "/v1/chat/completions"
		|| req.uri().query().is_some()
	{
		return Ok(error(StatusCode::NOT_FOUND, "not_found"));
	}
	if req
		.headers()
		.get("content-type")
		.and_then(|v| v.to_str().ok())
		!= Some("application/json")
	{
		return Ok(error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "json_required"));
	}
	let Ok(permit) = slots.try_acquire_owned() else {
		return Ok(error(StatusCode::TOO_MANY_REQUESTS, "backend_busy"));
	};
	let bytes = match tokio::time::timeout(
		Duration::from_secs(10),
		Limited::new(req.into_body(), MAX_INPUT).collect(),
	)
	.await
	{
		Ok(Ok(body)) => body.to_bytes(),
		_ => {
			return Ok(error(
				StatusCode::PAYLOAD_TOO_LARGE,
				"body_limit_or_timeout",
			));
		}
	};
	let request = match Request::parse(&bytes, &backend.config) {
		Ok(r) => r,
		Err(e) => return Ok(error(StatusCode::BAD_REQUEST, e.0)),
	};
	if !request.stream {
		let result = infer(&backend, &request).await;
		drop(permit);
		return Ok(match result {
			Ok(value) => response(StatusCode::OK, value),
			Err(e) => error(StatusCode::BAD_GATEWAY, e.0),
		});
	}
	// The stream owns inference and its permit. Dropping it on disconnect
	// cancels the child exchange; no detached task continues spending tokens.
	let stream = async_stream::stream! {
		let _permit=permit;
		yield Ok::<_,Infallible>(Frame::data(Bytes::from_static(b": cli pending\n\n")));
		let inference=infer(&backend,&request);
		tokio::pin!(inference);
		let result=loop {
			tokio::select! {
				result=&mut inference=>break result,
				_=tokio::time::sleep(Duration::from_secs(10))=>yield Ok(Frame::data(Bytes::from_static(b": cli pending\n\n"))),
			}
		};
		match result {
			Ok(value)=> {
				let mut chunk=value.clone();
				chunk["object"]=json!("chat.completion.chunk");
				let mut delta=chunk["choices"][0]["message"].take();
				if let Some(calls)=delta["tool_calls"].as_array_mut() { for (i,call) in calls.iter_mut().enumerate(){call["index"]=json!(i);} }
				chunk["choices"][0].as_object_mut().unwrap().remove("message");
				chunk["choices"][0]["delta"]=delta;
				yield Ok(Frame::data(Bytes::from(format!("data: {chunk}\n\ndata: [DONE]\n\n"))));
			},
			Err(e)=>yield Ok(Frame::data(Bytes::from(format!("data: {}\n\n",json!({"error":{"code":e.0,"message":e.0}}))))),
		}
	};
	let mut response = Response::new(StreamBody::new(stream).boxed_unsync());
	response
		.headers_mut()
		.insert("content-type", "text/event-stream".parse().unwrap());
	response
		.headers_mut()
		.insert("cache-control", "no-store".parse().unwrap());
	Ok(response)
}

pub async fn serve(
	listener: TcpListener,
	backend: Arc<Backend>,
	shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
	let slots = Arc::new(Semaphore::new(1));
	let connections = Arc::new(Semaphore::new(32));
	let mut tasks = JoinSet::new();
	tokio::pin!(shutdown);
	loop {
		tokio::select! {
			_=&mut shutdown=>break,
			Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
			accepted=listener.accept()=>{
				let (socket,peer)=accepted.map_err(|_|Failure("accept_failed"))?;
				if !peer.ip().is_loopback(){continue;}
				let Ok(permit)=connections.clone().try_acquire_owned() else {continue;};
				let backend=backend.clone();let slots=slots.clone();
				tasks.spawn(async move {
					let _permit=permit;
					let timeout=Duration::from_secs(backend.config.timeout_seconds+70);
					let service=service_fn(move|req|handle(req,backend.clone(),slots.clone()));
					let _=tokio::time::timeout(timeout,hyper::server::conn::http1::Builder::new().max_buf_size(16384).keep_alive(false).serve_connection(TokioIo::new(socket),service)).await;
				});
			}
		}
	}
	tasks.shutdown().await;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_integrations::cli_backend::config::{self, Config};
	use std::path::Path;
	#[tokio::test]
	async fn http_runtime_cli_roundtrip_json_sse_tool_history_and_admission() {
		super::super::examples::tests::committed_templates_match_the_real_registry_and_generated_definitions();
		let tmp = tempfile::tempdir().unwrap();
		let fixture =
			Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/cli-backends/claude.py");
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let port = listener.local_addr().unwrap().port();
		let mut config=Config::from_values(config::parse_env("AIDASH_CLI_PROVIDER=claude\nAIDASH_CLI_AUTH_MODE=native\nAIDASH_CLI_MODEL=fixture-model-1\nAIDASH_CLI_CONTEXT_WINDOW=8192").unwrap()).unwrap();
		config.port = port;
		config.executable = fixture;
		config.work_dir = tmp.path().into();
		let backend = Arc::new(Backend::fixture(config, &std::env::vars().collect()));
		let (stop, stopping) = tokio::sync::oneshot::channel();
		let task = tokio::spawn(serve(listener, backend, async {
			let _ = stopping.await;
		}));
		let client = reqwest::Client::builder().no_proxy().build().unwrap();
		let origin = format!("http://127.0.0.1:{port}");
		let api = reinhardt_test::client::APIClient::with_base_url(&origin);
		let health = api.get("/health").await.unwrap();
		assert_eq!(health.status_code(), 200);
		let health: serde_json::Value = health.json().unwrap();
		assert_eq!(health["inference_checked"], false);
		let specification =
			aidash_application::tools::builtins()["workspace_message"].specification();
		let mut request = json!({"model":"fixture-model-1","messages":[{"role":"user","content":"TOOL"}],"max_tokens":100,"tools":[{"type":"function","function":specification}]});
		let endpoint = format!("{origin}/v1/chat/completions");
		let first = client.post(&endpoint).json(&request).send().await.unwrap();
		assert_eq!(first.status(), 200);
		let first: serde_json::Value = first.json().await.unwrap();
		assert_eq!(first["choices"][0]["finish_reason"], "tool_calls");
		let message = first["choices"][0]["message"].clone();
		let id = message["tool_calls"][0]["id"].clone();
		request["messages"].as_array_mut().unwrap().extend([
			message,
			json!({"role":"tool","tool_call_id":id,"content":"done"}),
		]);
		let second: serde_json::Value = client
			.post(&endpoint)
			.json(&request)
			.send()
			.await
			.unwrap()
			.json()
			.await
			.unwrap();
		assert_eq!(second["choices"][0]["message"]["content"], "OK");
		request["stream"] = json!(true);
		let sse = client
			.post(&endpoint)
			.json(&request)
			.send()
			.await
			.unwrap()
			.text()
			.await
			.unwrap();
		assert!(sse.starts_with(": cli pending\n\n"));
		assert!(sse.contains("data: [DONE]"));
		assert!(sse.contains("chat.completion.chunk"));
		assert_eq!(
			client
				.post(&endpoint)
				.header("Origin", "https://evil.test")
				.json(&request)
				.send()
				.await
				.unwrap()
				.status(),
			403
		);
		request["stream"] = json!(false);
		request["messages"] = json!([{"role":"user","content":"WAIT"}]);
		let pending = client.post(&endpoint).json(&request);
		let waiting = tokio::spawn(async move { pending.send().await });
		tokio::time::timeout(Duration::from_secs(3), async {
			while std::fs::read_dir(tmp.path()).unwrap().count() == 0 {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
		assert_eq!(
			client
				.post(&endpoint)
				.json(&request)
				.send()
				.await
				.unwrap()
				.status(),
			429
		);
		// Dropping the HTTP client must cancel the child, not retain the slot
		// until the inference deadline. Then a new request must succeed.
		waiting.abort();
		assert!(waiting.await.unwrap_err().is_cancelled());
		tokio::time::timeout(Duration::from_secs(3), async {
			while std::fs::read_dir(tmp.path()).unwrap().count() != 0 {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
		request["messages"] = json!([{"role":"user","content":"Hi"}]);
		assert_eq!(
			client
				.post(&endpoint)
				.json(&request)
				.send()
				.await
				.unwrap()
				.status(),
			200
		);
		request["messages"] = json!([{"role":"user","content":"WAIT"}]);
		let pending = client.post(&endpoint).json(&request);
		let shutdown_request = tokio::spawn(async move { pending.send().await });
		tokio::time::timeout(Duration::from_secs(3), async {
			while std::fs::read_dir(tmp.path()).unwrap().count() == 0 {
				tokio::time::sleep(Duration::from_millis(10)).await;
			}
		})
		.await
		.unwrap();
		stop.send(()).unwrap();
		tokio::time::timeout(Duration::from_secs(3), task)
			.await
			.unwrap()
			.unwrap()
			.unwrap();
		assert!(shutdown_request.await.unwrap().is_err());
		assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
	}
}
