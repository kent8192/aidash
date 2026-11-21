//! Duty Trigger endpoints and the webhook that receives events (#1011).
use super::super::services::duty_events::{
	Operation, execute, receive, receive_from_node, sender_retries,
};
use crate::apps::identity::services::http_auth::InboundCall;
use crate::apps::workspaces::serializers::duties::NodeEventInput;
use crate::{authorization::identity::Actor, federation::Federation, http::json::Json};
use aidash_domain::duties::events::TriggerKind;
use reinhardt::http::ViewResult;
use reinhardt::{Path, Request, Response, StatusCode, delete, get, post};
use uuid::Uuid;

#[get(
	"/api/duties/{id}/triggers",
	name = "duty-triggers",
	auth = "protected"
)]
pub async fn list(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::List(id)).await)
}

#[post(
	"/api/duties/{id}/triggers",
	name = "duty-trigger-create",
	auth = "protected"
)]
pub async fn create(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
	Json(kind): Json<TriggerKind>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Create(id, kind)).await)
}

#[delete(
	"/api/duties/{id}/triggers/{trigger}",
	name = "duty-trigger-delete",
	auth = "protected"
)]
pub async fn remove(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path((id, trigger)): Path<(Uuid, Uuid)>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Delete(id, trigger)).await)
}

#[get("/api/duties/{id}/events", name = "duty-events", auth = "protected")]
pub async fn events(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	Path(id): Path<Uuid>,
) -> ViewResult<Response> {
	crate::http::json(execute(&runtime, actor, Operation::Events(id)).await)
}

/// The first of `names` the request carries, as text.
fn header(request: &Request, names: &[&str]) -> Option<String> {
	names
		.iter()
		.find_map(|name| request.headers.get(*name))
		.and_then(|value| value.to_str().ok())
		.map(str::to_owned)
}

/// A signed event for a webhook trigger. GitHub's `X-Hub-Signature-256`
/// and `X-Aidash-Signature` carry `sha256=<hex HMAC of the body>`. The same
/// body, or the same `X-GitHub-Delivery`, `X-Aidash-Delivery` or
/// `X-Request-Id`, within seven days is one delivery: it is answered with the
/// first one's receipt and starts nothing. An event that could not be taken
/// for now is answered 503, so the sender sends it again.
#[post("/hooks/duties/{trigger}", name = "duty-webhook", auth = "public")]
pub async fn webhook(
	#[inject] runtime: Federation,
	request: Request,
	Path(trigger): Path<Uuid>,
) -> ViewResult<Response> {
	let signature = header(&request, &["x-aidash-signature", "x-hub-signature-256"]);
	let delivery = header(
		&request,
		&["x-github-delivery", "x-aidash-delivery", "x-request-id"],
	);
	let received = receive(
		&runtime,
		trigger,
		(signature.as_deref(), delivery.as_deref()),
		request.body(),
	)
	.await;
	match received {
		// The event was not recorded, so the sender should send it again.
		Err(error) if sender_retries(&error) => {
			tracing::warn!(%error, %trigger, "a Duty webhook event waits for the sender to send it again");
			let body =
				serde_json::json!({"error": "the event could not be taken now; send it again"});
			Ok(Response::new(StatusCode::SERVICE_UNAVAILABLE)
				.with_body(serde_json::to_vec(&body).expect("error envelope serializes"))
				.with_header("Content-Type", "application/json")
				.with_header("Retry-After", "1"))
		}
		result => crate::http::json(result),
	}
}

/// An event another Node sends to a `node_event` trigger (#1013). The sending
/// Node and subject come from the verified Federation token, never the body.
#[post(
	"/federation/v0.1/scoped/duty-events",
	name = "duty-node-event",
	auth = "protected"
)]
pub async fn node_event(
	#[inject] runtime: Federation,
	#[inject] peer: InboundCall,
	Json(input): Json<NodeEventInput>,
) -> ViewResult<Response> {
	crate::http::json(receive_from_node(&runtime, peer.0, input.trigger_id, input.payload).await)
}
