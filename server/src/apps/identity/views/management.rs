//! Management HTTP endpoints.
use crate::authorization::identity::Actor;
use reinhardt::Depends;
use reinhardt::Response;
use reinhardt::get;
use reinhardt::http::ViewResult;

use crate::apps::identity::services::management::AuthorizationManagement;

#[get("/api/session", name = "session", auth = "protected")]
pub async fn session(
	#[inject] service: Depends<AuthorizationManagement>,
	#[inject] actor: Actor,
) -> ViewResult<Response> {
	crate::http::json(service.session(actor).await)
}
