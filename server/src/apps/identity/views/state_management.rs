//! HTTP conversion for the Run management contracts: a Tenant Subject's
//! diagnostics and control, and a Deployment Administrator's Safety Stop
//! (ADR 0101). The actor decides which contract a request is held to.
use crate::authorization::execution::management::{
	ManagementQuery, ManagementRequest, ManagementResult, ManagementView,
};
use crate::authorization::identity::Actor;
use aidash_domain::safety_stop::Actor as StopActor;
use uuid::Uuid;

/// The shared recovery token names nobody; a browser session names its Identity.
fn stop_actor(browser: Option<crate::dashboard_auth::BrowserOrigin>) -> StopActor {
	match browser {
		Some(origin) => StopActor::Browser {
			identity: origin.identity_id,
		},
		None => StopActor::SharedBearer,
	}
}

#[reinhardt::get(
	"/api/runs/{id}/management",
	name = "run-management-get",
	auth = "protected"
)]
pub async fn get(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	reinhardt::Query(query): reinhardt::Query<ManagementQuery>,
	reinhardt::Path(id): reinhardt::Path<Uuid>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(match actor {
		Actor::DeploymentAdministrator => {
			crate::authorization::execution::safety_stop::facts(&runtime, id, query.tenant)
				.await
				.map(ManagementView::Administrator)
		}
		Actor::Subject(_) if query.tenant.is_some() => Err(crate::Error::Invalid(
			"tenant is for Deployment Administrators".into(),
		)),
		actor => crate::authorization::execution::management::get(runtime, actor, id)
			.await
			.map(ManagementView::Subject),
	})
}
#[reinhardt::post(
	"/api/runs/{id}/management",
	name = "run-management-control",
	auth = "protected"
)]
pub async fn control(
	#[inject] runtime: crate::federation::Federation,
	#[inject] actor: crate::authorization::identity::Actor,
	#[inject] browser: Option<crate::dashboard_auth::BrowserOrigin>,
	request: reinhardt::Request,
	reinhardt::Path(id): reinhardt::Path<Uuid>,
	crate::http::json::Json(body): crate::http::json::Json<ManagementRequest>,
) -> reinhardt::http::ViewResult<reinhardt::Response> {
	crate::http::json(
		async {
			Ok(match actor {
				Actor::DeploymentAdministrator => {
					let input = body.for_administrator()?;
					let operation = operation_id(&request)?;
					ManagementResult::Administrator(
						crate::authorization::execution::safety_stop::stop(
							&runtime,
							stop_actor(browser),
							id,
							input,
							operation,
						)
						.await?,
					)
				}
				actor => ManagementResult::Subject(
					crate::authorization::execution::management::control(
						runtime,
						actor,
						id,
						body.for_subject()?,
					)
					.await?,
				),
			})
		}
		.await,
	)
}

/// The `Idempotency-Key` header: the command's operation ID.
fn operation_id(request: &reinhardt::Request) -> crate::Result<Uuid> {
	request
		.headers
		.get("idempotency-key")
		.and_then(|value| value.to_str().ok())
		.and_then(|value| Uuid::parse_str(value).ok())
		.ok_or_else(|| crate::Error::Invalid("Idempotency-Key must be a UUID".into()))
}
