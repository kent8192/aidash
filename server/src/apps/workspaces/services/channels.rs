//! Channel use cases resolved through the request DI context.
use crate::apps::workspaces::{access::Lease, attachments, history, posting, serializers::*};
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
	pub async fn post_message(
		&self,
		thread: Uuid,
		input: ChannelMessageInput,
	) -> Result<ChannelMessage> {
		crate::http::validate(&input)?;
		let mut lease =
			Lease::begin_message_create(&self.runtime.store, self.actor.clone(), thread).await?;
		let result = posting::post(
			&self.runtime.store,
			Some(&self.runtime),
			&mut lease,
			thread,
			input,
		)
		.await;
		let result = match result {
			Ok(mut message) => {
				let mut attachments: Vec<_> = message.attachments.iter_mut().collect();
				super::readability::annotate(
					&mut lease,
					thread,
					self.runtime.config.deployment.as_str(),
					&mut attachments,
				)
				.await
				.map(|()| message)
			}
			Err(error) => Err(error),
		};
		lease.finish(result).await
	}
	pub async fn history(
		&self,
		thread: Uuid,
		input: ChannelHistoryQuery,
	) -> Result<ChannelMessagePage> {
		crate::http::validate(&input)?;
		let mut lease = Lease::begin(
			&self.runtime.store,
			self.actor.clone(),
			thread,
			"thread.read",
		)
		.await?;
		lease.access_mut().lists = true;
		let result = history::page(&self.runtime.store, &mut lease, thread, input).await;
		let result = match result {
			Ok(mut page) => {
				let mut attachments: Vec<_> = page
					.messages
					.iter_mut()
					.flat_map(|message| message.attachments.iter_mut())
					.collect();
				super::readability::annotate(
					&mut lease,
					thread,
					self.runtime.config.deployment.as_str(),
					&mut attachments,
				)
				.await
				.map(|()| page)
			}
			Err(error) => Err(error),
		};
		lease.finish(result).await
	}
	pub async fn upload(
		&self,
		thread: Uuid,
		input: ChannelAttachmentUploadQuery,
		body: &[u8],
	) -> Result<ChannelAttachment> {
		crate::http::validate(&input)?;
		let mut lease = Lease::begin(
			&self.runtime.store,
			self.actor.clone(),
			thread,
			"message.create",
		)
		.await?;
		let result =
			attachments::upload(&self.runtime.store, &mut lease, thread, input, body).await;
		let result = match result {
			Ok(mut attachment) => super::readability::annotate(
				&mut lease,
				thread,
				self.runtime.config.deployment.as_str(),
				&mut [&mut attachment],
			)
			.await
			.map(|()| attachment),
			Err(error) => Err(error),
		};
		lease.finish(result).await
	}
	pub async fn describe(
		&self,
		thread: Uuid,
		attachment: Uuid,
		input: ChannelAttachmentDescription,
	) -> Result<ChannelAttachment> {
		crate::http::validate(&input)?;
		let mut lease = Lease::begin(
			&self.runtime.store,
			self.actor.clone(),
			thread,
			"message.create",
		)
		.await?;
		let result = attachments::describe(
			&self.runtime.store,
			&mut lease,
			thread,
			attachment,
			&input.description,
		)
		.await;
		lease.finish(result).await
	}
	pub(crate) async fn download(
		&self,
		thread: Uuid,
		attachment: Uuid,
	) -> Result<(ChannelAttachment, attachments::Download)> {
		let mut lease = Lease::begin(
			&self.runtime.store,
			self.actor.clone(),
			thread,
			"thread.read",
		)
		.await?;
		let result =
			attachments::download(&self.runtime.store, &mut lease, thread, attachment).await;
		lease.finish(result).await
	}
}
