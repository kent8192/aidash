//! The Home's side of a delivery from a Run executing for another Home (ADR 0139, decision 20).
//!
//! Three requests, each bound to the grant and its admission: `prepare` stages a manifest,
//! `chunks` stage the bytes, `commit` stores them. A delivery is named by the admission and the
//! Run's call key, so a repeated request finds what the first made. Staging, quota reservation
//! and publication are the existing `file-transfer/1` receiver's, run under the live grant's own
//! authority instead of a Peer Mapping; only where the bytes go is new: the Working Area of the
//! Run that delegated for `file_share`, and the Thread Attachments of a message the Home posts
//! for the Agent for `workspace_message`.
use super::*;
use crate::apps::execution::repositories::{home_delivery, transfer::HomeDelivery};
use crate::apps::identity::serializers::remote_execution_commands::{DeliveryChunk, DeliveryOpen};
use aidash_application::{
	capabilities::transfer::receiver,
	federation::{deliveries, inbound::Admitted},
	ports::capabilities::transfer::receiver::{DeliveryMode, ReceiverRepository as _},
};
use aidash_domain::{
	capabilities::{
		operations::MountedFile,
		sharing::Recipient,
		transfer::{Chunk, Description, Identity},
	},
	federation::delivery::{self, Limits, Parties, Purpose},
	registry::EntityRef,
};
use serde_json::json;
use std::collections::BTreeMap;

const TOOLS: [&str; 2] = ["file_share", "workspace_message"];

/// What the Home resolved from the live grant for one request of a delivery. None of it comes
/// from the request.
struct Admission {
	/// Where the delivery lands: for a share, the Working Area of the Run that delegated; for a
	/// message, which lands in no Area, this Node and the executing Agent, the grant's Task
	/// standing in for the thread.
	target: Recipient,
	/// The grant's root subject, whose authority the delivery is made under.
	subject: String,
	tenant: String,
	/// The Agent version executing the remote Run.
	agent: EntityRef,
	/// Who the Home names as the sender of a message: the executing Agent on its Node.
	owner: String,
	workspace: Uuid,
	limits: Limits,
	staging_seconds: i64,
}

fn mode(tool: &str) -> DeliveryMode {
	DeliveryMode {
		bound_operation: (tool == "file_share").then_some("file_share"),
	}
}

/// Admit one request: the live grant, its Binding, the tool the Run called and the task, then,
/// for a share, the Working Area of the delegating Run. A message is posted in the grant's
/// Workspace, so it needs no delegating Run and a grant a person made can send one (ADR 0139).
/// Opens and closes its own transaction.
async fn admit(
	f: &Federation,
	admitted: &Admitted,
	grant: Uuid,
	admission: Uuid,
	tool: &str,
) -> Result<Admission> {
	use aidash_application::ports::authorization::source::grants::GrantRepository as _;
	let node = admitted.sender.to_string();
	let (mut access, description) = super::super::delivery_lease(f, &node, grant).await?;
	let agent = description.inspection.agent.clone();
	let result: Result<Admission> = async {
		let request =
			aidash_application::authorization::peer::tool_request::requested_tools(&description)?;
		let task = {
			let mut scope = crate::bootstrap::remote_command_scope(f, &mut access);
			scope.bindings = Some(&description.inspection.binding_snapshot);
			deliveries::verify(
				&mut scope,
				&deliveries::Identity {
					grant_id: grant,
					admission_id: admission,
					node: &node,
					agent: &agent.id,
					version: &agent.version,
					request: request.as_ref(),
				},
				tool,
			)
			.await?
		};
		let row = crate::bootstrap::peer_home_execution_repository(f)
			.source_grant(grant, &node)
			.await?
			.ok_or(crate::Error::Forbidden)?;
		let node_id = f
			.config
			.deployment
			.node(admitted.receiver.node.clone())
			.to_string();
		let target = if mode(tool).into_area() {
			// The chain is the sending subjects and then the executor: the one before the
			// executor is the Run that delegated. A grant nobody's Run made has no such Run to
			// deliver to.
			let delegator = row
				.subject_chain
				.len()
				.checked_sub(2)
				.and_then(|index| row.subject_chain.get(index))
				.ok_or(crate::Error::Forbidden)?;
			let parent = task
				.parent_id
				.ok_or_else(|| crate::Error::NotFound("DELIVERY_NO_DELEGATING_RUN".into()))?;
			let (area, version) = home_delivery::delegating_area(
				&mut access,
				f.config.deployment.as_str(),
				parent,
				delegator,
			)
			.await?;
			Recipient {
				node_id,
				agent_id: area.agent_id.clone(),
				agent_version: version,
				thread_id: area.thread_id,
			}
		} else {
			Recipient {
				node_id,
				agent_id: agent.id.clone(),
				agent_version: agent.version.clone(),
				thread_id: task.id,
			}
		};
		let profile = &f.store.capabilities.0;
		Ok(Admission {
			target,
			subject: row.root_subject.clone(),
			tenant: row.tenant.clone(),
			owner: aidash_domain::agent_subject(&node, &agent.id, &agent.version),
			agent: EntityRef {
				id: agent.id.clone(),
				version: agent.version.clone(),
			},
			workspace: task.workspace_id,
			limits: Limits {
				files: profile.limits.share_files,
				file_bytes: profile.limits.share_file_bytes,
				bytes: profile.limits.share_bytes,
			},
			staging_seconds: profile.staging_seconds as i64,
		})
	}
	.await;
	access.finish(result).await
}

fn repository<'a>(
	f: &'a Federation,
	admitted: &Admitted,
	grant: Uuid,
	description: Description,
	tool: &str,
) -> crate::apps::execution::repositories::transfer::Repository<'a> {
	crate::bootstrap::home_delivery_repository(
		f,
		admitted.receiver.node.clone(),
		HomeDelivery {
			grant,
			description,
			mode: mode(tool),
		},
	)
}

/// The description a staged delivery was opened with, from its record.
async fn stored(f: &Federation, admitted: &Admitted, id: Uuid) -> Result<Option<Description>> {
	let base = crate::bootstrap::transfer_receiver_repository(f, admitted.receiver.node.clone());
	match base.incoming(id).await {
		Ok(record) => Ok(Some(serde_json::from_value(
			record.data["description"].clone(),
		)?)),
		Err(aidash_application::Error::NotFound(_)) => Ok(None),
		Err(error) => Err(error.into()),
	}
}

fn key(input_key: &str) -> Result<()> {
	if input_key.is_empty() || input_key.len() > 512 {
		return Err(crate::Error::Invalid("invalid delivery key".into()));
	}
	Ok(())
}

/// Stage a delivery's manifest. A repeat finds the delivery by its key and answers from it.
pub(crate) async fn prepare(
	f: Federation,
	admitted: Admitted,
	input: DeliveryOpen,
) -> Result<Value> {
	key(&input.key)?;
	let tool = input.purpose.tool();
	let admission = admit(&f, &admitted, input.grant_id, input.admission_id, tool).await?;
	deliveries::check_manifest(&input.purpose, &input.files, admission.limits)?;
	let id = delivery::delivery_id(input.admission_id, &input.key);
	let digest = delivery::input_digest(
		input.grant_id,
		input.admission_id,
		&input.key,
		&input.purpose,
		&input.files,
	);
	let node = admitted.sender.to_string();
	let description = match stored(&f, &admitted, id).await? {
		Some(existing) => {
			if existing.input_digest != digest {
				return Err(crate::Error::Conflict(
					"delivery key binds different input".into(),
				));
			}
			existing
		}
		None => delivery::description(
			id,
			digest.clone(),
			&input.files,
			Parties {
				source_node: node.clone(),
				tenant: admission.tenant,
				subject: admission.subject,
				agent: admission.agent,
				target: admission.target,
			},
			chrono::Utc::now() + chrono::Duration::seconds(admission.staging_seconds),
		),
	};
	let repository = repository(&f, &admitted, input.grant_id, description, tool);
	receiver::require_admission(&repository)?;
	receiver::prepare(
		&repository,
		&node,
		Identity {
			transfer_id: id,
			input_digest: digest,
		},
	)
	.await
	.map_err(Into::into)
}

/// Stage one chunk. The bytes of a chunk are checked against what the first request staged.
pub(crate) async fn chunk(
	f: Federation,
	admitted: Admitted,
	input: DeliveryChunk,
) -> Result<Value> {
	key(&input.key)?;
	if !TOOLS.contains(&input.tool.as_str()) {
		return Err(crate::Error::Invalid("unknown delivery tool".into()));
	}
	admit(
		&f,
		&admitted,
		input.grant_id,
		input.admission_id,
		&input.tool,
	)
	.await?;
	let id = delivery::delivery_id(input.admission_id, &input.key);
	let description = stored(&f, &admitted, id)
		.await?
		.ok_or_else(|| crate::Error::NotFound("transfer unavailable".into()))?;
	let digest = description.input_digest.clone();
	let node = admitted.sender.to_string();
	let repository = repository(&f, &admitted, input.grant_id, description, &input.tool);
	receiver::require_admission(&repository)?;
	receiver::chunk(
		&repository,
		&node,
		Chunk {
			transfer_id: id,
			input_digest: digest,
			file: input.file,
			offset: input.offset,
			data: input.data,
		},
	)
	.await
	.map_err(Into::into)
}

/// Store a staged delivery. A repeat returns the receipt the first commit made.
pub(crate) async fn commit(
	f: Federation,
	admitted: Admitted,
	input: DeliveryOpen,
) -> Result<Value> {
	key(&input.key)?;
	let tool = input.purpose.tool();
	let admission = admit(&f, &admitted, input.grant_id, input.admission_id, tool).await?;
	let id = delivery::delivery_id(input.admission_id, &input.key);
	let digest = delivery::input_digest(
		input.grant_id,
		input.admission_id,
		&input.key,
		&input.purpose,
		&input.files,
	);
	let description = stored(&f, &admitted, id)
		.await?
		.ok_or_else(|| crate::Error::NotFound("transfer unavailable".into()))?;
	if description.input_digest != digest {
		return Err(crate::Error::Forbidden);
	}
	let node = admitted.sender.to_string();
	let repository = repository(&f, &admitted, input.grant_id, description, tool);
	receiver::require_admission(&repository)?;
	let identity = Identity {
		transfer_id: id,
		input_digest: digest,
	};
	let Purpose::Message {
		content,
		descriptions,
	} = &input.purpose
	else {
		return receiver::commit(&repository, &node, identity)
			.await
			.map_err(Into::into);
	};
	let (description, files) =
		match receiver::staged_files(&repository, &node, identity.clone()).await? {
			receiver::Staged::Committed(view) => return Ok(view),
			receiver::Staged::Ready { description, files } => (description, files),
		};
	let receipt = post_message(
		&f,
		&node,
		&input,
		&admission,
		&description,
		content,
		descriptions,
		files,
	)
	.await?;
	receiver::settle(&repository, &node, identity, receipt)
		.await
		.map_err(Into::into)
}

/// Store a message delivery's files as Thread Attachments and post the message for the Agent, as
/// the local `workspace_message` does, in one transaction under the grant's authority. Every part
/// has a key derived from the delivery, so a commit that is retried after this one committed
/// finds the same attachments and the same message and posts nothing twice.
#[allow(clippy::too_many_arguments)]
async fn post_message(
	f: &Federation,
	node: &str,
	input: &DeliveryOpen,
	admission: &Admission,
	description: &Description,
	content: &str,
	descriptions: &BTreeMap<String, String>,
	files: Vec<(MountedFile, Vec<u8>)>,
) -> Result<Value> {
	use aidash_application::ports::authorization::commands::effects::RemoteCommandEffects as _;
	let id = description.transfer_id;
	let (access, _) = super::super::delivery_lease(f, node, input.grant_id).await?;
	let mut lease = crate::collaboration::access::Lease::from_access(access);
	let result: Result<Value> = async {
		let workspace = admission.workspace;
		let resource = lease.access_mut().workspace(workspace).await?;
		lease
			.access_mut()
			.require(&resource, "message.create")
			.await?;
		let mut attachments = Vec::with_capacity(files.len());
		let mut stored = Vec::with_capacity(files.len());
		for (index, (file, bytes)) in files.iter().enumerate() {
			let upload = crate::collaboration::ChannelAttachmentUploadQuery {
				filename: file.path.rsplit('/').next().unwrap_or("").to_owned(),
				media_type: file.media_type.clone(),
				idempotency_key: delivery::part_key(id, index),
				description: descriptions.get(&file.path).cloned(),
			};
			let attachment = crate::collaboration::attachments::upload_for_run(
				&f.store, &mut lease, workspace, upload, bytes, None,
			)
			.await?;
			attachments.push(attachment.id);
			stored.push(json!({
				"path":file.path,
				"attachment_id":attachment.id,
				"digest":file.digest,
				"size":file.size,
			}));
		}
		let message = {
			let mut scope = crate::bootstrap::channel_scope(&f.store, &mut lease);
			scope.agent = Some(admission.owner.clone());
			aidash_application::workspaces::channels::post(
				&mut scope,
				workspace,
				aidash_application::workspaces::channels::MessageSubmission {
					content,
					thread_id: None,
					idempotency_key: delivery::part_key(id, files.len()),
					attachment_ids: &attachments,
					citations: &[],
				},
			)
			.await?
			.message
			.id
		};
		// What the grant's Run wrote stays visible to its provenance after the lease closes.
		let mut outputs = crate::bootstrap::remote_command_scope(f, lease.access_mut());
		outputs
			.record_output(input.grant_id, workspace, "message", message)
			.await?;
		for attachment in &attachments {
			outputs
				.record_output(input.grant_id, workspace, "attachment", *attachment)
				.await?;
		}
		Ok(json!({
			"id":id,
			"node_id":admission.target.node_id,
			"message_id":message,
			"files":stored,
		}))
	}
	.await;
	lease.finish(result).await
}
