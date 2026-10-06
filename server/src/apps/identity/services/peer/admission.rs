//! HTTP conversion and DI composition call the shared receiver use cases.
pub(crate) use crate::apps::identity::serializers::peer_admission::{
	Admission, Input, MessageInput, RemoteExecutionControlInput,
};
use crate::{
	Result,
	authorization::{access::Access, remote::Description},
	domain::RunMetadata,
	federation::Federation,
	registry::AgentConfig,
	store::Store,
};
use aidash_application::authorization::peer::admission as app;
use http::HeaderMap;
use uuid::Uuid;
#[derive(Clone)]
pub struct PeerAdmissions {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide_admissions(#[inject] runtime: Federation) -> PeerAdmissions {
	PeerAdmissions { runtime }
}
#[derive(Clone)]
pub struct PeerExecutionManagement {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PeerExecutionManagement {
	tracing::trace!(
		service = "PeerExecutionManagement",
		"creating injectable service"
	);
	PeerExecutionManagement { runtime }
}
use reinhardt::injectable;
fn source(headers: &HeaderMap) -> Result<&str> {
	crate::apps::identity::services::http_auth::peer_node(headers)
}
impl PeerAdmissions {
	pub(crate) async fn admit(&self, headers: HeaderMap, input: Input) -> Result<Admission> {
		app::admit(
			&crate::bootstrap::peer_admission_repository(&self.runtime),
			source(&headers)?,
			input.grant_id,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}
	pub(crate) async fn verify(&self, headers: HeaderMap, id: Uuid) -> Result<bool> {
		app::verify(
			&crate::bootstrap::peer_admission_repository(&self.runtime),
			source(&headers)?,
			id,
		)
		.await
		.map_err(Into::into)
	}
}
pub(crate) async fn message(
	f: Federation,
	headers: HeaderMap,
	id: Uuid,
	input: MessageInput,
) -> Result<crate::authorization::remote::execution::RemoteExecutionMessageReceipt> {
	let message = aidash_domain::federation::execution::admission::Message {
		id: input.message.id,
		content: input.message.content,
	};
	app::message(
		&crate::bootstrap::peer_admission_repository(&f),
		source(&headers)?,
		id,
		input.grant_id,
		message,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn control(
	f: Federation,
	headers: HeaderMap,
	id: Uuid,
	input: RemoteExecutionControlInput,
) -> Result<crate::authorization::remote::execution::RemoteExecutionActivation> {
	app::control(
		&crate::bootstrap::peer_admission_repository(&f),
		source(&headers)?,
		id,
		input.grant_id,
		input.action,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn status(
	f: Federation,
	headers: HeaderMap,
	input: Input,
) -> Result<Option<crate::authorization::remote::execution::RemoteExecutionActivation>> {
	app::status(
		&crate::bootstrap::peer_admission_repository(&f),
		source(&headers)?,
		input.grant_id,
	)
	.await
	.map(|r| r.map(Into::into))
	.map_err(Into::into)
}
pub(crate) async fn activate(
	f: Federation,
	headers: HeaderMap,
	id: Uuid,
	input: Input,
) -> Result<crate::authorization::remote::execution::RemoteExecutionActivation> {
	app::activate(
		&crate::bootstrap::peer_admission_repository(&f),
		source(&headers)?,
		id,
		input.grant_id,
	)
	.await
	.map(Into::into)
	.map_err(Into::into)
}
pub(crate) async fn run_grant(store: &Store, run: &RunMetadata) -> Result<Option<Uuid>> {
	app::run_grant(&crate::bootstrap::peer_admission_records(store), run)
		.await
		.map_err(Into::into)
}
pub(crate) async fn worker_lease(
	f: &Federation,
	run: &RunMetadata,
) -> Result<Option<(Access, AgentConfig)>> {
	app::worker_lease(&crate::bootstrap::peer_admission_repository(f), run)
		.await
		.map(|r| r.map(|(scope, agent)| (*scope.access, agent)))
		.map_err(Into::into)
}
pub(crate) async fn leaf_lease(
	f: &Federation,
	source: &str,
	grant: Uuid,
	admission: Uuid,
) -> Result<(Access, Description)> {
	app::leaf_lease(
		&crate::bootstrap::peer_admission_repository(f),
		source,
		grant,
		admission,
	)
	.await
	.map(|(scope, d)| (*scope.access, d))
	.map_err(Into::into)
}
