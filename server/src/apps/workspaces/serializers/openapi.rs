//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::federation::remote::serializers::actions::SentResponse;
use crate::apps::workspaces::serializers::channels::ChannelAttachment;
use crate::apps::workspaces::serializers::channels::ChannelAttachmentUploadQuery;
use crate::apps::workspaces::serializers::channels::ChannelHistoryQuery;
use crate::apps::workspaces::serializers::channels::ChannelMessage;
use crate::apps::workspaces::serializers::channels::ChannelMessageInput;
use crate::apps::workspaces::serializers::channels::ChannelMessagePage;
use crate::apps::workspaces::serializers::channels::ChannelThread;
use crate::apps::workspaces::serializers::channels::ChannelThreadInput;
use crate::apps::workspaces::serializers::entities::NewTask;
use crate::apps::workspaces::serializers::entities::Task;
use crate::apps::workspaces::serializers::entities::Workspace;
use crate::apps::workspaces::serializers::entities::WorkspaceSnapshot;
use crate::apps::workspaces::serializers::management::AbandonInput;
use crate::apps::workspaces::serializers::management::ConversationInput;
use crate::apps::workspaces::serializers::management::MessageInput;
use crate::apps::workspaces::serializers::management::StateInput;
use crate::apps::workspaces::serializers::management::WorkspaceInput;
use crate::apps::workspaces::serializers::tasks::ConversationResponse;
use crate::apps::workspaces::serializers::tasks::PageQuery;
use crate::apps::workspaces::serializers::tasks::TaskPage;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, TaskPage>(
		document,
		views::management::task_list,
		200,
		"application/json",
	)?;
	contracts.query::<_, PageQuery>(document, views::management::task_list)?;
	contracts.response::<_, Workspace>(
		document,
		views::management::workspace_create,
		200,
		"application/json",
	)?;
	contracts.request::<_, WorkspaceInput>(document, views::management::workspace_create)?;
	contracts.response::<_, WorkspaceSnapshot>(
		document,
		views::management::workspace_get,
		200,
		"application/json",
	)?;
	contracts.path(document, views::management::workspace_get, &["Uuid"])?;
	contracts.response::<_, Workspace>(
		document,
		views::management::workspace_update,
		200,
		"application/json",
	)?;
	contracts.request::<_, StateInput>(document, views::management::workspace_update)?;
	contracts.path(document, views::management::workspace_update, &["Uuid"])?;
	contracts.response::<_, Task>(
		document,
		views::management::task_create,
		200,
		"application/json",
	)?;
	contracts.request::<_, NewTask>(document, views::management::task_create)?;
	contracts.path(document, views::management::task_create, &["Uuid"])?;
	contracts.response::<_, SentResponse>(
		document,
		views::management::message_create,
		200,
		"application/json",
	)?;
	contracts.request::<_, MessageInput>(document, views::management::message_create)?;
	contracts.path(document, views::management::message_create, &["Uuid"])?;
	contracts.response::<_, Task>(
		document,
		views::management::task_abandon,
		200,
		"application/json",
	)?;
	contracts.request::<_, AbandonInput>(document, views::management::task_abandon)?;
	contracts.path(document, views::management::task_abandon, &["Uuid"])?;
	contracts.response::<_, ConversationResponse>(
		document,
		views::management::conversation_create,
		200,
		"application/json",
	)?;
	contracts.request::<_, ConversationInput>(document, views::management::conversation_create)?;
	contracts.response::<_, ChannelThread>(
		document,
		views::channels::channel_thread_create,
		200,
		"application/json",
	)?;
	contracts.request::<_, ChannelThreadInput>(document, views::channels::channel_thread_create)?;
	contracts.path(document, views::channels::channel_thread_create, &["Uuid"])?;
	contracts.response::<_, ChannelMessage>(
		document,
		views::channels::channel_message_create,
		200,
		"application/json",
	)?;
	contracts
		.request::<_, ChannelMessageInput>(document, views::channels::channel_message_create)?;
	contracts.path(document, views::channels::channel_message_create, &["Uuid"])?;
	contracts.response::<_, ChannelMessagePage>(
		document,
		views::channels::channel_message_history,
		200,
		"application/json",
	)?;
	contracts
		.query::<_, ChannelHistoryQuery>(document, views::channels::channel_message_history)?;
	contracts.path(
		document,
		views::channels::channel_message_history,
		&["Uuid"],
	)?;
	contracts.response::<_, ChannelAttachment>(
		document,
		views::channels::channel_attachment_upload,
		200,
		"application/json",
	)?;
	contracts.binary_request(document, views::channels::channel_attachment_upload)?;
	contracts.query::<_, ChannelAttachmentUploadQuery>(
		document,
		views::channels::channel_attachment_upload,
	)?;
	contracts.path(
		document,
		views::channels::channel_attachment_upload,
		&["Uuid"],
	)?;
	contracts.empty(document, views::channels::channel_attachment_download, 200)?;
	contracts.path(
		document,
		views::channels::channel_attachment_download,
		&["Uuid", "Uuid"],
	)?;
	Ok(())
}
