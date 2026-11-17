//! Channel request and response contracts.

pub(crate) mod activity;
pub mod entities;

pub use channels::{
	ChannelAttachment, ChannelAttachmentDescription, ChannelAttachmentUploadQuery,
	ChannelHistoryQuery, ChannelMessage, ChannelMessageInput, ChannelMessagePage, ChannelThread,
	ChannelThreadInput,
};

pub mod channels;

pub mod services;

pub mod management;

pub mod tasks;

pub(crate) mod openapi;

pub mod schedule_settings;
pub(crate) mod schedules;
pub(crate) mod scratch;
