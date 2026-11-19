//! Persistent records owned by the collaboration app.
mod catalog;
pub(crate) mod child_flags;
mod inspection;
mod mutations;

mod artifacts;
pub use artifacts::Artifact;
mod channel_attachments;
pub use channel_attachments::ChannelAttachment;
mod channel_message_context;
pub use channel_message_context::ChannelMessageContext;
mod conversations;
pub use conversations::Conversation;
mod messages;
pub use messages::Message;
mod message_file_versions;
pub use message_file_versions::MessageFileVersion;
mod task_dependencies;
pub use task_dependencies::TaskDependency;
mod tasks;
pub use tasks::Task;
mod threads;
pub use threads::Thread;

pub mod states;

mod scratch_spaces;
pub use scratch_spaces::{ScratchSpace, ScratchSpaceMember, ScratchSpaceSettings};

mod duties;
pub use duties::{
	DutyBookingRecord, DutyEventRecord, DutyRecord, DutyScriptRunRecord, DutyScriptVersionRecord,
	DutyTickRecord, DutyTriggerRecord,
};
