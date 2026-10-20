//! A stateful streamable-HTTP MCP server on loopback for the tests that drive a Run through an
//! Agent Plugin's remote server: `initialize` issues a session, a request without a live session is
//! refused, `DELETE` ends it, and every request is recorded.
#![allow(dead_code)] // Each integration target consumes a different part of this fixture.
use crate::common::upstream_fixtures::reply;
use http::{Method, StatusCode};
use reinhardt::{Request, Response, ServerRouter, test::stub::StubRouter};
use serde_json::{Value, json};
use std::{
	collections::HashSet,
	sync::{
		Arc, Mutex,
		atomic::{AtomicUsize, Ordering::SeqCst},
	},
};

pub const PATH: &str = "/mcp";

#[derive(Default)]
pub struct FakeMcp {
	tools: Mutex<Vec<Value>>,
	live: Mutex<HashSet<String>>,
	issued: AtomicUsize,
	seen: Mutex<Vec<(String, Value)>>,
	deleted: Mutex<Vec<String>>,
}

/// A tool whose input schema is `{"type":"object","properties":{"q":{"type":"string"}}}`.
pub fn tool(name: &str) -> Value {
	json!({"name": name, "description": format!("Does {name}."), "inputSchema": schema()})
}

pub fn schema() -> Value {
	json!({"type": "object", "properties": {"q": {"type": "string"}}})
}

fn answer(id: Value, result: Value) -> Response {
	Response::ok()
		.with_json(&json!({"jsonrpc": "2.0", "id": id, "result": result}))
		.unwrap()
}

impl FakeMcp {
	pub fn new(tools: Vec<Value>) -> Arc<Self> {
		Arc::new(Self {
			tools: Mutex::new(tools),
			..Self::default()
		})
	}

	/// The requests of one MCP method, as their `params`, in arrival order.
	pub fn requests(&self, method: &str) -> Vec<Value> {
		self.seen
			.lock()
			.unwrap()
			.iter()
			.filter(|(seen, _)| seen == method)
			.map(|(_, params)| params.clone())
			.collect()
	}

	/// The `tools/call` requests: the tool and its arguments.
	pub fn calls(&self) -> Vec<(String, Value)> {
		self.requests("tools/call")
			.into_iter()
			.map(|params| {
				(
					params["name"].as_str().unwrap_or_default().to_owned(),
					params["arguments"].clone(),
				)
			})
			.collect()
	}

	pub fn sessions_issued(&self) -> usize {
		self.issued.load(SeqCst)
	}

	/// The sessions a client ended with `DELETE`.
	pub fn ended(&self) -> Vec<String> {
		self.deleted.lock().unwrap().clone()
	}

	fn handle(&self, request: &Request) -> Response {
		let Ok(body) = request.json::<Value>() else {
			return Response::new(StatusCode::BAD_REQUEST);
		};
		let method = body["method"].as_str().unwrap_or_default().to_owned();
		let session = request
			.headers
			.get("mcp-session-id")
			.and_then(|value| value.to_str().ok())
			.map(str::to_owned);
		self.seen
			.lock()
			.unwrap()
			.push((method.clone(), body["params"].clone()));
		let id = body.get("id").cloned();
		if method == "initialize" {
			let session = format!("session-{}", self.issued.fetch_add(1, SeqCst) + 1);
			self.live.lock().unwrap().insert(session.clone());
			return answer(
				id.unwrap_or(Value::Null),
				json!({
					"protocolVersion": body["params"]["protocolVersion"],
					"capabilities": {"tools": {}},
					"serverInfo": {"name": "fake", "version": "1.0.0"}
				}),
			)
			.with_header("mcp-session-id", &session);
		}
		match session {
			Some(session) if self.live.lock().unwrap().contains(&session) => {}
			Some(_) => return Response::new(StatusCode::NOT_FOUND),
			None => return Response::new(StatusCode::BAD_REQUEST),
		}
		let Some(id) = id else {
			return Response::new(StatusCode::ACCEPTED);
		};
		match method.as_str() {
			"tools/list" => answer(id, json!({"tools": self.tools.lock().unwrap().clone()})),
			"tools/call" => answer(
				id,
				json!({
					"content": [{"type": "text", "text": body["params"]["arguments"].to_string()}],
					"isError": false
				}),
			),
			other => panic!("unexpected MCP method {other:?}"),
		}
	}

	/// Routes for `PATH`. The caller adds its own routes to the same server, such as a model.
	pub fn routes(self: &Arc<Self>, routes: StubRouter) -> StubRouter {
		let posted = self.clone();
		let ended = self.clone();
		routes
			.route(
				PATH,
				Method::POST,
				reply(move |request: Request| {
					let response = posted.handle(&request);
					async move { response }
				}),
			)
			// The client opens a standalone event stream; this server has none to offer.
			.route(
				PATH,
				Method::GET,
				reply(|_request: Request| async { Response::new(StatusCode::METHOD_NOT_ALLOWED) }),
			)
			.route(
				PATH,
				Method::DELETE,
				reply(move |request: Request| {
					if let Some(session) = request
						.headers
						.get("mcp-session-id")
						.and_then(|value| value.to_str().ok())
					{
						ended.live.lock().unwrap().remove(session);
						ended.deleted.lock().unwrap().push(session.to_owned());
					}
					async { Response::ok() }
				}),
			)
	}
}

/// Only for readers of this module: the router type the fixtures serve.
pub type Router = Arc<ServerRouter>;
