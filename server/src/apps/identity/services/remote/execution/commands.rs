//! HTTP commands adapt the shared application use case and commit its source authority lease.
use super::*;
use aidash_application::authorization::commands::effects;
use aidash_application::federation::inbound::Admitted;
use aidash_application::ports::authorization::commands::effects::RemoteCommandEffects as _;
pub(crate) async fn handle(f: Federation, admitted: Admitted, input: Input) -> Result<Value> {
	let node = &admitted.sender.to_string();
	let (mut access, description) =
		super::super::description_lease_mode(&f, node, input.grant_id, true).await?;
	access.worker();
	access.read_grant = Some(input.grant_id);
	let agent = &description.inspection.agent;
	// The Request this Home asks of the sender now, with the grant's Run-only answers: the
	// Description was rebuilt for this command, so a change to either reaches it at once.
	let request = match aidash_application::authorization::peer::tool_request::requested_tools(
		&description,
	) {
		Ok(request) => request,
		Err(error) => return access.finish(Err(error.into())).await,
	};
	let mut command_scope = crate::bootstrap::remote_command_scope(&f, &mut access);
	command_scope.bindings = Some(&description.inspection.binding_snapshot);
	let command = aidash_application::authorization::commands::Command {
		grant_id: input.grant_id,
		admission_id: input.admission_id,
		operation: &input.operation,
		data: &input.data,
		node,
		agent: &agent.id,
		version: &agent.version,
		request: request.as_ref(),
	};
	if effects::is_onward(command_scope.local_node(), &input.operation, &input.data) {
		// Multi-hop delegation (#654): check the child under this lease, release
		// it, and only then prepare and activate this Home's own grant, which
		// talks to the target Node. A repeat of a command already handed on
		// answers what it answered then.
		enum Next {
			Replay(Value),
			HandOn(effects::Onward, Vec<String>),
		}
		let next = match effects::onward(&mut command_scope, command)
			.await
			.map_err(crate::Error::from)
		{
			Ok(effects::OnwardStep::Replay(result)) => Ok(Next::Replay(result)),
			Ok(effects::OnwardStep::HandOn(onward)) => {
				crate::apps::identity::repositories::remote_grants::persistence::subject_chain(
					&mut access,
					input.grant_id,
				)
				.await
				.map(|chain| Next::HandOn(onward, chain))
			}
			Err(error) => Err(error),
		};
		let identity = access.identity.clone();
		return match access.finish(next).await? {
			Next::Replay(result) => Ok(result),
			Next::HandOn(onward, chain) => {
				let root = identity.subject.clone();
				let home = HandOnHome {
					federation: &f,
					identity,
				};
				effects::hand_on(&home, &root, input.grant_id, chain, onward)
					.await
					.map_err(Into::into)
			}
		};
	}
	let result = effects::execute(&mut command_scope, command, agent)
		.await
		.map_err(Into::into);
	if let Err(error) = &result {
		tracing::warn!(operation=%input.operation,error=%error,"scoped home command rejected");
	}
	access.finish(result).await
}

/// The Home's side of handing an onward delegation on (#654, ADR 0109), as
/// the grant's root person acting for its whole chain. Each step is a short
/// transaction of its own, so none spans the peer calls of preparing and
/// activating the next grant.
struct HandOnHome<'a> {
	federation: &'a Federation,
	identity: crate::authorization::identity::SubjectIdentity,
}

/// One check of [`HandOnHome`]: the asking grant's liveness and the chain's
/// authority, in one transaction under the chain's subjects.
struct HandOnAccess(crate::authorization::access::Access);

#[async_trait::async_trait]
impl effects::HandOnRepository for HandOnHome<'_> {
	type Scope = HandOnAccess;

	async fn begin(&self, chain: &[String]) -> aidash_application::Result<HandOnAccess> {
		let mut access =
			crate::authorization::access::Access::begin(&self.federation.store, &self.identity)
				.await?;
		access.subjects = chain.to_vec();
		Ok(HandOnAccess(access))
	}

	async fn delegate(
		&self,
		chain: Vec<String>,
		onward: &effects::Onward,
	) -> aidash_application::Result<aidash_domain::federation::Delegation> {
		let repository = crate::apps::identity::repositories::home_execution::Repository {
			runtime: self.federation.clone(),
			actor: Some(crate::authorization::identity::Actor::Subject(
				self.identity.clone(),
			)),
			worker: None,
			chain: Some(chain),
		};
		aidash_application::authorization::home::delegate_with_model(
			&repository,
			onward.child,
			&onward.node,
			&onward.agent,
			true,
			onward.task_model.as_ref(),
		)
		.await
	}

	async fn save_replay(
		&self,
		grant: Uuid,
		replay: &effects::OnwardReplay,
		result: &Value,
	) -> aidash_application::Result<()> {
		let mut access =
			crate::authorization::access::Access::begin(&self.federation.store, &self.identity)
				.await?;
		let saved =
			crate::apps::identity::repositories::remote_grants::persistence::save_onward_replay(
				&mut access,
				grant,
				(&replay.request_key, &replay.digest),
				result,
			)
			.await;
		access.finish(saved).await.map_err(Into::into)
	}
}

#[async_trait::async_trait]
impl effects::HandOnScope for HandOnAccess {
	async fn live(&mut self, grant: Uuid) -> aidash_application::Result<bool> {
		crate::apps::identity::repositories::remote_grants::persistence::live(&mut self.0, grant)
			.await
			.map_err(Into::into)
	}

	async fn task_read(&mut self, id: Uuid) -> aidash_application::Result<aidash_domain::Task> {
		self.0.task_read(id).await.map_err(Into::into)
	}

	async fn task_resource(
		&mut self,
		task: &aidash_domain::Task,
	) -> aidash_application::Result<aidash_domain::policy::Resource> {
		self.0.task_resource(task).await.map_err(Into::into)
	}

	fn node_resource(&self, node: &str) -> aidash_domain::policy::Resource {
		self.0
			.resource("node", node, serde_json::json!({"remote_node": node}))
	}

	async fn require(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		action: &str,
	) -> aidash_application::Result<()> {
		self.0.require(resource, action).await.map_err(Into::into)
	}

	async fn finish(
		self,
		result: aidash_application::Result<()>,
	) -> aidash_application::Result<()> {
		self.0
			.finish(result.map_err(crate::Error::from))
			.await
			.map_err(Into::into)
	}
}

/// Fetch only for the direct delegate, under both the live grant and the
/// caller's current Peer Mapping. Bytes are never persisted on the receiver.
pub(crate) async fn thread_attachment(
	f: Federation,
	admitted: Admitted,
	input: AttachmentInput,
) -> Result<AttachmentResponse> {
	use base64::Engine as _;
	if admitted.subject().is_none() {
		return Err(crate::Error::Forbidden);
	}
	let (mut access, description) = super::super::description_lease_mode(
		&f,
		&admitted.sender.to_string(),
		input.grant_id,
		false,
	)
	.await?;
	access.worker();
	access.read_grant = Some(input.grant_id);
	let thread = description.task.thread_id;
	let mut lease = crate::collaboration::access::Lease::from_access(access);
	let result: Result<AttachmentResponse> = async {
		let binding = super::binding(lease.access_mut(), input.grant_id)
			.await?
			.ok_or(crate::Error::Forbidden)?;
		if binding.admission_id != input.admission_id || binding.task_id != description.task.id {
			return Err(crate::Error::Forbidden);
		}
		let mut mapped = crate::authorization::peer::access(&f, &admitted).await?;
		let readable = mapped
			.thread_record(thread, "attachment", input.attachment_id)
			.await
			.map(|_| ());
		mapped.finish(readable).await?;
		let record = lease
			.access_mut()
			.thread_record(thread, "attachment", input.attachment_id)
			.await?;
		// Attachment reads must remain visible to grant provenance after the lease closes.
		{
			use aidash_application::ports::authorization::journals::{
				ReadJournalScope as _, ReadMembership,
			};
			let mut journal = crate::bootstrap::run_visibility_scope(lease.access_mut());
			match journal.membership() {
				(None, Some(grant)) => {
					journal
						.record_sources(
							ReadMembership::RemoteGrant(grant),
							thread,
							&[("attachment".into(), input.attachment_id)],
						)
						.await?
				}
				_ => return Err(crate::Error::Forbidden),
			}
		}
		let (attachment, bytes) = crate::collaboration::attachments::download(
			&f.store,
			&mut lease,
			thread,
			input.attachment_id,
		)
		.await?;
		let digest = crate::apps::execution::repositories::capability_objects::digest(&bytes);
		if record["sha256"] != digest {
			return Err(crate::Error::Conflict(
				"attachment integrity check failed".into(),
			));
		}
		if !super::super::live(lease.access_mut(), input.grant_id).await? {
			return Err(crate::Error::Forbidden);
		}
		let content_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
		drop(bytes);
		Ok(AttachmentResponse {
			filename: attachment.filename,
			media_type: attachment.media_type,
			size_bytes: attachment.size_bytes,
			message_id: serde_json::from_value(record["message_id"].clone())?,
			sha256: digest,
			content_base64,
		})
	}
	.await;
	lease.finish(result).await
}
#[derive(Clone)]
pub struct RemoteCommands {
	pub(crate) runtime: Federation,
}
#[reinhardt::injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> RemoteCommands {
	RemoteCommands { runtime }
}
pub(crate) use super::deliveries::{
	chunk as delivery_chunk, commit as delivery_commit, prepare as delivery_prepare,
};
pub(crate) use crate::apps::identity::serializers::remote_execution_commands::AttachmentInput;
pub(crate) use crate::apps::identity::serializers::remote_execution_commands::AttachmentResponse;
pub(crate) use crate::apps::identity::serializers::remote_execution_commands::Input;
pub(crate) use crate::apps::identity::serializers::remote_execution_commands::{
	DeliveryChunk, DeliveryOpen,
};
