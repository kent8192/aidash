//! The Home's side of a delivery from a Run executing for another Home (ADR 0139, decision 20).
//!
//! Three requests, each bound to the grant and its admission: `prepare` stages a manifest,
//! `chunks` stage the bytes and `commit` posts the message with its Thread Attachments.
//! The admission and the Run's call key identify a delivery across retries.
use super::*;
use crate::apps::execution::repositories::transfer;
use crate::apps::identity::serializers::remote_execution_commands::{DeliveryChunk, DeliveryOpen};
use aidash_application::{
	capabilities::transfer::receiver,
	federation::{deliveries, inbound::Admitted},
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

/// What the Home resolved from the live grant for one request of a delivery. None of it comes
/// from the request.
struct Admission {
	/// The Home Node, the executing Agent and the grant's Thread.
	target: Recipient,
	/// The grant's root subject, whose authority the delivery is made under.
	subject: String,
	tenant: String,
	/// The Agent version executing the remote Run.
	agent: EntityRef,
	/// Who the Home names as the sender of a message: the executing Agent on its Node.
	owner: String,
	thread: Uuid,
	task_id: Uuid,
	limits: Limits,
	staging_seconds: i64,
}

/// Resolve the message destination from the live grant and its admission.
async fn admit(
	f: &Federation,
	admitted: &Admitted,
	grant: Uuid,
	admission: Uuid,
	tool: &str,
) -> Result<Admission> {
	if !matches!(tool, "thread_message" | "completion_attachments") {
		return Err(crate::Error::Forbidden);
	}
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
		if tool == "completion_attachments" {
			let resource = access.task_resource(&task).await?;
			access.require(&resource, "task.complete").await?;
			let thread = access.thread(task.thread_id).await?;
			access.require(&thread, "message.create").await?;
		}
		let row = crate::bootstrap::peer_home_execution_repository(f)
			.source_grant(grant, &node)
			.await?
			.ok_or(crate::Error::Forbidden)?;
		let node_id = f
			.config
			.deployment
			.node(admitted.receiver.node.clone())
			.to_string();
		let target = Recipient {
			node_id,
			agent_id: agent.id.clone(),
			agent_version: agent.version.clone(),
			thread_id: task.thread_id,
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
			thread: task.thread_id,
			task_id: task.id,
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
	completion: bool,
) -> transfer::Repository<'a> {
	let node = admitted.receiver.node.clone();
	let address = f.config.deployment.node(node.clone()).to_string();
	transfer::Repository {
		federation: f,
		node,
		address,
		grant,
		description,
		completion,
	}
}

/// The description a staged delivery was opened with, from its record.
async fn stored(f: &Federation, admitted: &Admitted, id: Uuid) -> Result<Option<Description>> {
	transfer::incoming(f, &admitted.receiver.node, id).await
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
	let repository = repository(
		&f,
		&admitted,
		input.grant_id,
		description,
		tool == "completion_attachments",
	);
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
	let repository = repository(
		&f,
		&admitted,
		input.grant_id,
		description,
		input.tool == "completion_attachments",
	);
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
	let repository = repository(
		&f,
		&admitted,
		input.grant_id,
		description,
		tool == "completion_attachments",
	);
	receiver::require_admission(&repository)?;
	let identity = Identity {
		transfer_id: id,
		input_digest: digest,
	};
	if let Purpose::CompletionAttachments { message_id } = &input.purpose {
		let staged = receiver::staged_parts(&repository, &node, identity.clone()).await?;
		let (description, files) = match staged {
			receiver::Parts::Committed(view) => return Ok(view),
			receiver::Parts::Ready { description, files } => (description, files),
		};
		let receipt = stage_completion(
			&f,
			&node,
			&input,
			&admission,
			&description,
			*message_id,
			files,
		)
		.await?;
		return receiver::settle(&repository, &node, identity, receipt)
			.await
			.map_err(Into::into);
	}
	let Purpose::Message {
		content,
		descriptions,
	} = &input.purpose
	else {
		return Err(crate::Error::Forbidden);
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
/// the local `thread_message` does, in one transaction under the grant's authority. Every part
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
		let thread = admission.thread;
		let resource = lease.access_mut().thread(thread).await?;
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
				&f.store, &mut lease, thread, upload, bytes, None,
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
			aidash_application::threads::channels::post(
				&mut scope,
				thread,
				aidash_application::threads::channels::MessageSubmission {
					content,
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
			.record_output(input.grant_id, thread, "message", message)
			.await?;
		for attachment in &attachments {
			outputs
				.record_output(input.grant_id, thread, "attachment", *attachment)
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

async fn stage_completion(
	f: &Federation,
	node: &str,
	input: &DeliveryOpen,
	admission: &Admission,
	description: &Description,
	message: Uuid,
	parts: Vec<Vec<MountedFile>>,
) -> Result<Value> {
	use crate::apps::execution::capabilities::serializers::contracts::FileEntry;
	use reinhardt::query::{
		Alias, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	let (mut access, _) = super::super::delivery_lease(f, node, input.grant_id).await?;
	let result = async {
		let resource = access.thread(admission.thread).await?;
		access.require(&resource, "message.create").await?;
		// Serialize receiver publication by the Task before releasing the
		// staged transfer's reservation. The stable attachment IDs make a
		// repeated commit safe after a lost response.
		let task = crate::apps::workspaces::repositories::goals::locked_task(&mut **access.tx, admission.task_id).await?;
		let task_resource = access.task_resource(&task).await?;
		access.require(&task_resource, "task.complete").await?;
		let mut files = Vec::new();
		let mut credit = 0_i64;
		for (file, parts) in description.files.iter().zip(parts) {
			let id = Uuid::new_v5(&message, file.path.as_bytes());
			let saved: Option<Uuid> = crate::database::native::query_scalar(
				&Query::select().column(Alias::new("id")).from(Alias::new("channel_attachments"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.and_where(Expr::col("thread_id").eq(Expr::value(admission.thread)))
					.and_where(Expr::col("uploaded_by").eq(admission.owner.clone()))
					.and_where(Expr::col("sha256").eq(file.digest.clone()))
					.lock(LockType::Update).to_string(PostgresQueryBuilder),
			).scalar_optional(&mut **access.tx).await?;
			credit += file.size as i64;
			if saved.is_none() {
				let mut object = f.store.capabilities.begin_object_converting(
					&mut access, None, "thread_attachment", file.size, credit,
				).await?;
				for part in parts {
					let bytes = f.store.capabilities.read(&mut access, &FileEntry::from(part)).await?;
					object.write_block(&bytes).await?;
				}
				let (object, digest) = object.finish(&mut access, Some(&file.digest)).await?;
				crate::apps::workspaces::models::ChannelAttachment::stage_completion(
					&mut **access.tx,
					crate::apps::workspaces::models::channel_attachments::CompletionAttachment {
						id, thread: admission.thread, sender: &admission.owner, message,
						filename: file.path.rsplit('/').next().unwrap_or(&file.path),
						media_type: &file.media_type, digest: &digest, size: file.size, object,
					},
				).await?;
			}
			files.push(json!({"path":file.path,"attachment_id":id,"digest":file.digest,"size":file.size}));
		}
		Ok(json!({"id":description.transfer_id,"node_id":admission.target.node_id,"message_id":message,"files":files}))
	}.await;
	access.finish(result).await
}
