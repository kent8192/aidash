//! Duty Trigger endpoints and the webhook that receives events (#1011).
use super::super::services::duty_events::{Operation, execute, receive};
use crate::{authorization::identity::Actor, federation::Federation, http::json::Json};
use aidash_domain::duties::events::TriggerKind;
use reinhardt::http::ViewResult;
use reinhardt::{Path, Request, Response, delete, get, post};
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

/// A signed event for a webhook trigger. GitHub's `X-Hub-Signature-256`
/// and `X-Aidash-Signature` carry `sha256=<hex HMAC of the body>`.
#[post("/hooks/duties/{trigger}", name = "duty-webhook", auth = "public")]
pub async fn webhook(
	#[inject] runtime: Federation,
	request: Request,
	Path(trigger): Path<Uuid>,
) -> ViewResult<Response> {
	let signature = ["x-aidash-signature", "x-hub-signature-256"]
		.into_iter()
		.find_map(|name| request.headers.get(name))
		.and_then(|value| value.to_str().ok())
		.map(str::to_owned);
	crate::http::json(receive(&runtime, trigger, signature.as_deref(), request.body()).await)
}
