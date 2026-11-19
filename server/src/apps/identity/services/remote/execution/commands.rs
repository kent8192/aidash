//! HTTP commands adapt the shared application use case and commit its source authority lease.
use super::*;
use aidash_application::authorization::commands::effects;
use aidash_application::federation::inbound::Admitted;
use aidash_application::ports::authorization::commands::effects::RemoteCommandEffects as _;
pub(crate) async fn handle(f: Federation, admitted: Admitted, input: Input) -> Result<Value> {
	let node = &admitted.sender.to_string();
	let mut revision_race = false;
	let (mut access, description) =
		super::super::description_lease_mode(&f, node, input.grant_id, true, &mut revision_race)
			.await?;
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
		// talks to the target Node.
		let checked = effects::onward(&mut command_scope, command)
			.await
			.map_err(Into::into);
		let checked = match checked {
			Ok(onward) => {
				crate::apps::identity::repositories::remote_grants::persistence::subject_chain(
					&mut access,
					input.grant_id,
				)
				.await
				.map(|chain| (onward, chain))
			}
			Err(error) => Err(error),
		};
		let identity = access.identity.clone();
		let (onward, chain) = access.finish(checked).await?;
		return hand_on(&f, identity, chain, onward).await;
	}
	let result = effects::execute(&mut command_scope, command, agent)
		.await
		.map_err(Into::into);
	if let Err(error) = &result {
		tracing::warn!(operation=%input.operation,error=%error,"scoped home command rejected");
	}
	access.finish(result).await
}

/// Hands a child Task of a Remote Grant Run on to another Node through a
/// Remote Grant of this Home (#654, ADR 0109). It acts under the grant's whole
/// chain, never as its root person alone: every subject of the chain, the
/// receiver's executor included, must be allowed `task.delegate` on the child
/// and `federation.execute` on the target Node by this Node's policy; the
/// grant it prepares carries that chain, a prepared grant is reused only for
/// exactly that chain, and the target decides by its own consent.
async fn hand_on(
	f: &Federation,
	identity: crate::authorization::identity::SubjectIdentity,
	chain: Vec<String>,
	onward: effects::Onward,
) -> Result<Value> {
	if chain.len() < 2 || chain.first() != Some(&identity.subject) {
		return Err(crate::Error::Forbidden);
	}
	// The executor that asks must itself be allowed to hand the Task there.
	let mut access = crate::authorization::access::Access::begin(&f.store, &identity).await?;
	access.subjects = chain.clone();
	let checked = async {
		let task = access.task_read(onward.child).await?;
		let resource = access.task_resource(&task).await?;
		access.require(&resource, "task.delegate").await?;
		let node = access.resource(
			"node",
			&onward.node,
			serde_json::json!({"remote_node": onward.node}),
		);
		access.require(&node, "federation.execute").await
	}
	.await;
	access.finish(checked).await?;
	let repository = crate::apps::identity::repositories::home_execution::Repository {
		runtime: f.clone(),
		actor: Some(crate::authorization::identity::Actor::Subject(identity)),
		worker: None,
		chain: Some(chain),
	};
	let delegation = aidash_application::authorization::home::delegate(
		&repository,
		onward.child,
		&onward.node,
		&onward.agent,
		true,
	)
	.await
	.map_err(crate::Error::from)?;
	Ok(serde_json::to_value(delegation)?)
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
	let mut revision_race = false;
	let (mut access, description) = super::super::description_lease_mode(
		&f,
		&admitted.sender.to_string(),
		input.grant_id,
		false,
		&mut revision_race,
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
