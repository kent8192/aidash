//! Federation inside one process (ADR 0064). A request to a Node of this
//! Deployment never leaves it: the same `accept` that decides a request off
//! the wire decides it here, with `Proof::InProcess` in place of a token, and
//! the same service function that the HTTP view calls answers it. Nothing but
//! this module builds `Proof::InProcess`.
//!
//! The sender, its Node and the subject it acts for are what the server
//! already authenticated: the `Route` was resolved from the caller's own Tenant
//! and policy. The caller's transaction is never open while this runs, because
//! peer I/O only starts after the application has made its durable writes.
use crate::{
	Error, Result,
	apps::federation::transactions::{
		Manifest, serializers::authority::Preflight, services::protocol::Transactions,
	},
	federation::Federation,
	transactions::gate::ReadLease,
};
use aidash_application::federation::inbound::{Admitted, Inbound, Proof, RequestDigest, accept};
use aidash_domain::{federation::route::Route, semantic::Failure};
use http::HeaderMap;
use reinhardt::Response;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::pin::Pin;
use uuid::Uuid;

/// What a handler answered, in the terms the transport ports use for both paths.
pub(crate) struct Outcome {
	pub status: u16,
	pub body: Value,
	pub semantic_reason: Option<Failure>,
	pub transaction_pending: bool,
	pub run_message_pending: bool,
}

impl Outcome {
	fn ok<T: Serialize>(value: &T) -> Self {
		match serde_json::to_value(value) {
			Ok(body) => Self {
				status: 200,
				body,
				semantic_reason: None,
				transaction_pending: false,
				run_message_pending: false,
			},
			Err(error) => Error::from(error).into(),
		}
	}
}

impl From<Error> for Outcome {
	fn from(error: Error) -> Self {
		response(error.http_response())
	}
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
	headers
		.get(name)
		.and_then(|value| value.to_str().ok())
		.map(str::to_owned)
}

/// The same fields the HTTP client reads off a response.
fn response(response: Response) -> Outcome {
	Outcome {
		status: response.status.as_u16(),
		semantic_reason: header(&response.headers, "x-aidash-semantic-reason")
			.and_then(|reason| serde_json::from_value(json!(reason)).ok()),
		transaction_pending: header(&response.headers, "x-aidash-transaction-pending")
			.is_some_and(|value| value == "1"),
		run_message_pending: response
			.headers
			.contains_key("x-aidash-run-message-pending"),
		body: serde_json::from_slice(&response.body).unwrap_or(Value::Null),
	}
}

fn respond<T: Serialize>(result: Result<T>) -> Outcome {
	match result {
		Ok(value) => Outcome::ok(&value),
		Err(error) => error.into(),
	}
}

fn decode<T: DeserializeOwned>(body: Option<&Value>) -> Result<T> {
	serde_json::from_value(body.cloned().unwrap_or(Value::Null))
		.map_err(|_| Error::Invalid("invalid JSON request".into()))
}

fn uuid(segment: &str) -> Result<Uuid> {
	Uuid::parse_str(segment).map_err(|_| Error::Invalid("invalid identifier".into()))
}

/// Decode the request body or answer 400, like the HTTP extractor.
macro_rules! input {
	($body:expr, $ty:ty) => {
		match decode::<$ty>($body) {
			Ok(value) => value,
			Err(error) => return error.into(),
		}
	};
}

/// The same as `input!`, then the validation the view runs before its service.
macro_rules! checked {
	($body:expr, $ty:ty) => {{
		let value = input!($body, $ty);
		if let Err(error) = crate::http::validate(&value) {
			return error.into();
		}
		value
	}};
}

macro_rules! id {
	($segment:expr) => {
		match uuid($segment) {
			Ok(id) => id,
			Err(error) => return error.into(),
		}
	};
}

/// Answer `method path` at the Node `route.target` for the sender `route`
/// names. A refusal is the same status the wire would answer.
pub(crate) async fn call(
	f: &Federation,
	route: &Route,
	method: &str,
	path: &str,
	body: Option<&Value>,
) -> Result<Outcome> {
	let path = path.split_once('?').map_or(path, |(path, _)| path);
	let bytes = body
		.map(serde_json::to_vec)
		.transpose()?
		.unwrap_or_default();
	let admitted = match accept(
		Inbound {
			receiver: route.target.clone(),
			sender: route.sender.clone(),
			subject: route.subject.clone().unwrap_or_default(),
			proof: Proof::InProcess,
		},
		&RequestDigest {
			method,
			path,
			body: &bytes,
		},
		f,
	)
	.await
	{
		Ok(admitted) => admitted,
		Err(error) => return Ok(Error::from(error).into()),
	};
	// Visibility fences atomic transactions for everything except their own
	// protocol, as the peer routes do over HTTP.
	let _visibility = if path.starts_with("/transactions/") {
		None
	} else {
		match ReadLease::begin(&f.store).await {
			Ok(lease) => Some(lease),
			Err(error) => return Ok(error.into()),
		}
	};
	Ok(dispatch(f, admitted, method, path, body).await)
}

/// Build a route's future on this function's frame and move it to the heap.
/// Built inline, an unoptimized `dispatch` keeps every route's future on its
/// own poll frame, about 1 MiB of a debug stack under each in-process peer
/// request (#645).
#[inline(never)]
fn boxed<F: Future>(future: impl FnOnce() -> F) -> Pin<Box<F>> {
	Box::pin(future())
}

/// One arm per peer route, calling what the route's view calls.
async fn dispatch(
	f: &Federation,
	admitted: Admitted,
	method: &str,
	path: &str,
	body: Option<&Value>,
) -> Outcome {
	use crate::apps::identity::serializers::{
		peer::DiscoveryInput, peer_admission, peer_execution::InspectInput, peer_reads,
		remote::VerifyInput,
	};
	use crate::authorization::{peer as identity_peer, remote::execution as remote_execution};
	let f = f.clone();
	let segments = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
	match (method, segments.as_slice()) {
		("POST", ["scoped", "execution", "admissions"]) => {
			let input = checked!(body, peer_admission::Input);
			let service = crate::apps::identity::peer::admission::PeerAdmissions { runtime: f };
			respond(boxed(|| service.admit(admitted, input)).await)
		}
		("POST", ["scoped", "execution", "admissions", id, "verify"]) => {
			let id = id!(id);
			let service = crate::apps::identity::peer::admission::PeerAdmissions { runtime: f };
			respond(boxed(|| service.verify(admitted, id)).await)
		}
		("POST", ["scoped", "execution", "admissions", id, "activate"]) => {
			let id = id!(id);
			let input = checked!(body, peer_admission::Input);
			respond(boxed(|| identity_peer::admission::activate(f, admitted, id, input)).await)
		}
		("POST", ["scoped", "execution", "admissions", id, "control"]) => {
			let id = id!(id);
			let input = checked!(body, peer_admission::RemoteExecutionControlInput);
			respond(boxed(|| identity_peer::admission::control(f, admitted, id, input)).await)
		}
		("POST", ["scoped", "execution", "admissions", id, "messages"]) => {
			let id = id!(id);
			let input = checked!(body, peer_admission::MessageInput);
			respond(boxed(|| identity_peer::admission::message(f, admitted, id, input)).await)
		}
		("POST", ["scoped", "execution", "status"]) => {
			let input = checked!(body, peer_admission::Input);
			respond(boxed(|| identity_peer::admission::status(f, admitted, input)).await)
		}
		("POST", ["scoped", "execution", "inspect"]) => {
			let input = input!(body, InspectInput);
			let service = crate::apps::identity::peer::execution::PeerExecution { runtime: f };
			respond(boxed(|| service.inspect(admitted, input)).await)
		}
		("POST", ["scoped", "previews", "tickets"]) => {
			let input = input!(body, aidash_domain::federation::preview::TicketRequest);
			respond(
				boxed(|| {
					crate::apps::execution::capabilities::services::federation_previews::issue(
						f, admitted, input,
					)
				})
				.await,
			)
		}
		("POST", ["scoped", "previews", "inspect"]) => {
			let input = input!(body, aidash_domain::federation::preview::InspectRequest);
			respond(
				boxed(|| {
					crate::apps::execution::capabilities::services::federation_previews::inspect(
						f, admitted, input,
					)
				})
				.await,
			)
		}
		("POST", ["scoped", "previews", "retain"]) => {
			let input = input!(body, aidash_domain::federation::preview::RetainRequest);
			respond(
				boxed(|| {
					crate::apps::execution::capabilities::services::federation_previews::retain(
						f, admitted, input,
					)
				})
				.await,
			)
		}
		("POST", ["scoped", "previews", "release"]) => {
			let input = input!(body, aidash_domain::federation::preview::ReleaseRequest);
			respond(
				boxed(|| {
					crate::apps::execution::capabilities::services::federation_previews::release(
						f, admitted, input,
					)
				})
				.await,
			)
		}
		("POST", ["scoped", "execution", "commands"]) => {
			let input = checked!(body, remote_execution::commands::Input);
			respond(boxed(|| remote_execution::commands::handle(f, admitted, input)).await)
		}
		("POST", ["scoped", "execution", "grants", "activation"]) => {
			let input = input!(body, VerifyInput);
			respond(boxed(|| remote_execution::activation_binding(f, admitted, input)).await)
		}
		(
			"POST",
			[
				"scoped",
				"execution",
				"grants",
				operation @ ("describe" | "verify" | "snapshot"),
			],
		) => {
			let input = input!(body, VerifyInput);
			let service = crate::apps::identity::remote::RemoteGrants { runtime: f };
			match *operation {
				"describe" => respond(boxed(|| service.describe(admitted, input)).await),
				"verify" => respond(boxed(|| service.verify(admitted, input)).await),
				_ => respond(boxed(|| service.snapshot(admitted, input)).await),
			}
		}
		("POST", ["scoped", "registry", "verify"]) => {
			let input = input!(body, peer_reads::VerifyInput);
			let service = crate::apps::identity::peer::reads::PeerReads { runtime: f };
			respond(boxed(|| service.verify(admitted, input)).await)
		}
		("POST", ["scoped", "dependencies", "verify"]) => {
			let input = input!(body, crate::apps::identity::peer::dependencies::Input);
			respond(
				boxed(|| crate::apps::identity::peer::dependencies::verify(f, admitted, input))
					.await,
			)
		}
		("POST", ["scoped", "discover"]) => {
			let input = input!(body, DiscoveryInput);
			respond(boxed(|| identity_peer::discover_for(f, admitted, input)).await)
		}
		("POST", ["scoped", "graph"]) => {
			let input = checked!(body, identity_peer::graph::GraphRequest);
			respond(boxed(|| identity_peer::graph::project(f, admitted, input)).await)
		}
		("POST", ["scoped", "semantic", "query"]) => {
			let input = input!(body, crate::semantic::remote::Operation);
			respond(
				boxed(|| crate::authorization::remote::semantic::search(f, admitted, input)).await,
			)
		}
		("POST", ["scoped", "semantic", "verify-operation"]) => {
			let input = input!(body, crate::semantic::remote::Operation);
			respond(boxed(|| identity_peer::semantic::verify_operation(f, admitted, input)).await)
		}
		("POST", ["scoped", "files", operation]) => {
			boxed(|| files(f, admitted, operation, body)).await
		}
		(
			"POST",
			[
				"scoped",
				"generation",
				operation @ ("cancel" | "describe" | "prepare"),
			],
		) => {
			use crate::generation::foreign;
			let input = input!(body, foreign::Reference);
			match *operation {
				"cancel" => respond(boxed(|| foreign::cancel_at(f, admitted, input)).await),
				"describe" => respond(boxed(|| foreign::describe(f, admitted, input)).await),
				_ => respond(boxed(|| foreign::prepare(f, admitted, input)).await),
			}
		}
		("POST", ["scoped", "usage", "finalize"]) => {
			use crate::generation::remote::{dispatch, protocol};
			let input = input!(body, dispatch::FinalizeInput);
			respond(boxed(|| protocol::finalize(f, admitted, input)).await)
		}
		("POST", ["scoped", "usage", operation @ ("reserve" | "verify")]) => {
			use crate::generation::remote::{dispatch, protocol};
			let input = input!(body, dispatch::Input);
			match *operation {
				"reserve" => respond(boxed(|| protocol::reserve(f, admitted, input)).await),
				_ => respond(boxed(|| protocol::verify(f, admitted, input)).await),
			}
		}
		(
			"POST",
			[
				"transactions",
				operation @ ("reserve" | "prepare" | "finish"),
			],
		) => {
			let manifest = input!(body, Manifest);
			let service = Transactions::for_peers(f);
			match *operation {
				"reserve" => respond(boxed(|| service.reserve(admitted, manifest)).await),
				"prepare" => respond(boxed(|| service.prepare(admitted, manifest)).await),
				_ => respond(boxed(|| service.finish(admitted, manifest)).await),
			}
		}
		("POST", ["transactions", operation @ ("preflight" | "access")]) => {
			let input = input!(body, Preflight);
			let service = Transactions::for_peers(f);
			match *operation {
				"preflight" => respond(boxed(|| service.preflight(admitted, input)).await),
				_ => respond(boxed(|| service.read_access(admitted, input)).await),
			}
		}
		("GET", ["transactions", id, "decision"]) => {
			let id = id!(id);
			let service = Transactions::for_peers(f);
			respond(boxed(|| service.decision(admitted, id)).await)
		}
		("GET", ["transactions", id, "authority"]) => {
			let id = id!(id);
			let service = Transactions::for_peers(f);
			respond(boxed(|| service.authority_ticket(admitted, id)).await)
		}
		_ => Error::NotFound("not found".into()).into(),
	}
}

async fn files(
	f: Federation,
	admitted: Admitted,
	operation: &str,
	body: Option<&Value>,
) -> Outcome {
	use crate::capabilities::transfer::{self, Chunk, Identity, Requester, receiver};
	match operation {
		"describe" => {
			let input = input!(body, Identity);
			respond(boxed(|| transfer::describe(f, admitted, input)).await)
		}
		"negotiate" => {
			let input = input!(body, Requester);
			respond(boxed(|| receiver::negotiate(f, admitted, input)).await)
		}
		"recipients" => {
			let input = input!(body, Requester);
			respond(boxed(|| receiver::recipients(f, admitted, input)).await)
		}
		"chunk" => {
			let input = input!(body, Chunk);
			respond(boxed(|| receiver::chunk(f, admitted, input)).await)
		}
		"prepare" => {
			let input = input!(body, Identity);
			respond(boxed(|| receiver::prepare(f, admitted, input)).await)
		}
		"commit" => {
			let input = input!(body, Identity);
			respond(boxed(|| receiver::commit(f, admitted, input)).await)
		}
		"status" => {
			let input = input!(body, Identity);
			respond(boxed(|| receiver::status(f, admitted, input)).await)
		}
		_ => Error::NotFound("not found".into()).into(),
	}
}

/// The error an atomic transaction RPC maps a status to, the same for either
/// path.
pub(crate) fn classify_transaction<T: DeserializeOwned>(
	outcome: Outcome,
) -> aidash_application::Result<T> {
	use aidash_application::Error as E;
	let status = outcome.status;
	if (200..300).contains(&status) {
		return serde_json::from_value(outcome.body).map_err(Into::into);
	}
	let label = http::StatusCode::from_u16(status)
		.map(|status| status.to_string())
		.unwrap_or_else(|_| status.to_string());
	Err(match status {
		400 | 422 | 404 | 405 => {
			E::Invalid(format!("transaction participant rejected request: {label}"))
		}
		401 => E::Unauthorized,
		403 => E::Forbidden,
		409 => E::Conflict("transaction participant rejected its state precondition".into()),
		503 if outcome.transaction_pending => E::TransactionPending,
		_ => E::External(format!("transaction participant returned {label}")),
	})
}
