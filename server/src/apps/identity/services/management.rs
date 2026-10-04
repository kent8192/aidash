//! Management use cases.
use crate::{
	Result, apps::identity::serializers::session::AccessProfile,
	apps::identity::serializers::session::SessionResponse, authorization::identity::Actor,
	federation::Federation,
};
use reinhardt::injectable;

#[derive(Clone)]
pub struct AuthorizationManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> AuthorizationManagement {
	AuthorizationManagement { runtime }
}
impl AuthorizationManagement {
	pub(crate) async fn session(&self, actor: Actor) -> Result<SessionResponse> {
		let f = self.runtime.clone();
		let access = match actor {
			Actor::Operator => AccessProfile::Operator,
			Actor::Subject(identity) => AccessProfile::Subject {
				tenant: identity.tenant,
				subject: identity.subject,
			},
		};
		Ok(SessionResponse {
			access,
			node_id: f.config.node_id,
		})
	}
}
