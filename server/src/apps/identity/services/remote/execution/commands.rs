//! HTTP commands adapt the shared application use case and commit its source authority lease.
use super::*;
use aidash_application::federation::inbound::Admitted;
pub(crate) async fn handle(f: Federation, admitted: Admitted, input: Input) -> Result<Value> {
	let node = &admitted.sender.to_string();
	let mut revision_race = false;
	let (mut access, description) =
		super::super::description_lease_mode(&f, node, input.grant_id, true, &mut revision_race)
			.await?;
	access.worker();
	access.read_grant = Some(input.grant_id);
	let agent = &description.inspection.agent;
	let mut command_scope = crate::bootstrap::remote_command_scope(&f, &mut access);
	command_scope.bindings = Some(&description.inspection.binding_snapshot);
	let result = aidash_application::authorization::commands::effects::execute(
		&mut command_scope,
		aidash_application::authorization::commands::Command {
			grant_id: input.grant_id,
			admission_id: input.admission_id,
			operation: &input.operation,
			data: &input.data,
			node,
			agent: &agent.id,
			version: &agent.version,
		},
		agent,
	)
	.await
	.map_err(Into::into);
	if let Err(error) = &result {
		tracing::warn!(operation=%input.operation,error=%error,"scoped home command rejected");
	}
	access.finish(result).await
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
	let mut mapped = crate::authorization::peer::access(&f, &admitted).await?;
	let result: Result<AttachmentResponse> = async {
		let mut revision_race = false;
		let (mut access, description) = super::super::description_lease_mode(
			&f,
			&admitted.sender.to_string(),
			input.grant_id,
			true,
			&mut revision_race,
		)
		.await?;
		access.worker();
		let workspace = description.task.workspace_id;
		let mut lease = crate::collaboration::access::Lease::from_access(access);
		let result: Result<AttachmentResponse> = async {
			let binding = super::binding(lease.access_mut(), input.grant_id)
				.await?
				.ok_or(crate::Error::Forbidden)?;
			if binding.admission_id != input.admission_id || binding.task_id != description.task.id
			{
				return Err(crate::Error::Forbidden);
			}
			mapped
				.workspace_record(workspace, "attachment", input.attachment_id)
				.await?;
			let record = lease
				.access_mut()
				.workspace_record(workspace, "attachment", input.attachment_id)
				.await?;
			let (attachment, bytes) = crate::collaboration::attachments::download(
				&f.store,
				&mut lease,
				workspace,
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
			Ok(AttachmentResponse {
				filename: attachment.filename,
				media_type: attachment.media_type,
				size_bytes: attachment.size_bytes,
				message_id: serde_json::from_value(record["message_id"].clone())?,
				sha256: digest,
				content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
			})
		}
		.await;
		lease.finish(result).await
	}
	.await;
	mapped.finish(result).await
}
#[derive(Clone)]
pub struct RemoteCommands {
	pub(crate) runtime: Federation,
}
#[reinhardt::injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> RemoteCommands {
	RemoteCommands { runtime }
}
pub(crate) use crate::apps::identity::serializers::remote_execution_commands::AttachmentInput;
pub(crate) use crate::apps::identity::serializers::remote_execution_commands::AttachmentResponse;
pub(crate) use crate::apps::identity::serializers::remote_execution_commands::Input;
