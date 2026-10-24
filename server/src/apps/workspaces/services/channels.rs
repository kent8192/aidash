//! Channel use cases resolved through the request DI context.
use crate::apps::workspaces::{access::Lease, attachments, history, serializers::*, threads};
use crate::{Result, authorization::identity::Actor, federation::Federation};
use reinhardt::injectable;
use uuid::Uuid;

#[derive(Clone)]
pub struct Channels {
	runtime: Federation,
	actor: Actor,
}

#[injectable(scope = "request")]
pub async fn channels(#[inject] runtime: Federation, #[inject] actor: Actor) -> Channels {
	Channels { runtime, actor }
}

impl Channels {
	pub async fn create_thread(
		&self,
		workspace: Uuid,
		input: ChannelThreadInput,
	) -> Result<ChannelThread> {
		let mut lease = Lease::begin(
			&self.runtime.store,
			self.actor.clone(),
			workspace,
			"message.create",
		)
		.await?;
		let result = threads::create(
			&self.runtime.store,
			&mut lease,
			workspace,
			input.root_message_id,
		)
		.await;
		lease.finish(result).await
	}
	pub async fn post_message(
		&self,
		workspace: Uuid,
		input: ChannelMessageInput,
	) -> Result<ChannelMessage> {
		crate::http::validate(&input)?;
		let mut lease = if input.thread_id.is_some() {
			Lease::begin(
				&self.runtime.store,
				self.actor.clone(),
				workspace,
				"message.create",
			)
			.await?
		} else {
			Lease::begin_message_create(&self.runtime.store, self.actor.clone(), workspace).await?
		};
		let result = threads::post(&self.runtime.store, &mut lease, workspace, input).await;
		lease.finish(result).await
	}
	pub async fn history(
		&self,
		workspace: Uuid,
		input: ChannelHistoryQuery,
	) -> Result<ChannelMessagePage> {
		crate::http::validate(&input)?;
		let mut lease = Lease::begin(
			&self.runtime.store,
			self.actor.clone(),
			workspace,
			"workspace.read",
		)
		.await?;
		let result = history::page(&self.runtime.store, &mut lease, workspace, input).await;
		lease.finish(result).await
	}
	pub async fn upload(
		&self,
		workspace: Uuid,
		input: ChannelAttachmentUploadQuery,
		body: &[u8],
	) -> Result<ChannelAttachment> {
		crate::http::validate(&input)?;
		let mut lease = Lease::begin(
			&self.runtime.store,
			self.actor.clone(),
			workspace,
			"message.create",
		)
		.await?;
		let result =
			attachments::upload(&self.runtime.store, &mut lease, workspace, input, body).await;
		lease.finish(result).await
	}
	pub async fn download(
		&self,
		workspace: Uuid,
		attachment: Uuid,
	) -> Result<(ChannelAttachment, Vec<u8>)> {
		let mut lease = Lease::begin(
			&self.runtime.store,
			self.actor.clone(),
			workspace,
			"workspace.read",
		)
		.await?;
		let result =
			attachments::download(&self.runtime.store, &mut lease, workspace, attachment).await;
		lease.finish(result).await
	}
}
