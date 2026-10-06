//! Durable channel conversations; message creation does not authorize execution.
#[path = "access.rs"]
pub mod access;
#[path = "attachments.rs"]
pub mod attachments;
#[path = "history.rs"]
pub mod history;
#[path = "threads.rs"]
pub mod threads;

pub use crate::apps::workspaces::serializers::{
	ChannelAttachment, ChannelAttachmentUploadQuery, ChannelHistoryQuery, ChannelMessage,
	ChannelMessageInput, ChannelMessagePage, ChannelThread, ChannelThreadInput,
};

#[path = "entities.rs"]
pub mod entities;
