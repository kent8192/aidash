//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::federation::remote::serializers::actions::SentResponse;
use crate::apps::workspaces::serializers::channels::ChannelAttachment;
use crate::apps::workspaces::serializers::channels::ChannelAttachmentDescription;
use crate::apps::workspaces::serializers::channels::ChannelAttachmentUploadQuery;
use crate::apps::workspaces::serializers::channels::ChannelHistoryQuery;
use crate::apps::workspaces::serializers::channels::ChannelMessage;
use crate::apps::workspaces::serializers::channels::ChannelMessageInput;
use crate::apps::workspaces::serializers::channels::ChannelMessagePage;
use crate::apps::workspaces::serializers::entities::NewTask;
use crate::apps::workspaces::serializers::entities::Task;
use crate::apps::workspaces::serializers::entities::Thread;
use crate::apps::workspaces::serializers::entities::ThreadSnapshot;
use crate::apps::workspaces::serializers::management::AbandonInput;
use crate::apps::workspaces::serializers::management::MessageInput;
use crate::apps::workspaces::serializers::management::ThreadInput;
use crate::apps::workspaces::serializers::management::TitleInput;
use crate::apps::workspaces::serializers::scratch::{
	ScratchCreate, ScratchLimits, ScratchList, ScratchMember, ScratchView,
};
use crate::apps::workspaces::serializers::tasks::PageQuery;
use crate::apps::workspaces::serializers::tasks::TaskPage;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	// Duties (#1011, from the Schedules of #557).
	{
		use super::duties::{
			DeletedView, DetailView, DutyListQuery, DutyView, NoticesView, RunNowView,
			SuccessorInput,
		};
		use aidash_domain::duties::{Definition, Patch};
		let json = "application/json";
		contracts.path(document, views::duties::thread_list, &["Uuid"])?;
		contracts.response::<_, Vec<DutyView>>(document, views::duties::thread_list, 200, json)?;
		contracts.path(document, views::duties::create, &["Uuid"])?;
		contracts.request::<_, Definition>(document, views::duties::create)?;
		contracts.response::<_, DutyView>(document, views::duties::create, 200, json)?;
		contracts.query::<_, DutyListQuery>(document, views::duties::list)?;
		contracts.response::<_, Vec<DutyView>>(document, views::duties::list, 200, json)?;
		contracts.response::<_, NoticesView>(document, views::duties::notices, 200, json)?;
		contracts.path(document, views::duties::read, &["Uuid"])?;
		contracts.response::<_, DetailView>(document, views::duties::read, 200, json)?;
		contracts.path(document, views::duties::update, &["Uuid"])?;
		contracts.request::<_, Patch>(document, views::duties::update)?;
		contracts.response::<_, DutyView>(document, views::duties::update, 200, json)?;
		contracts.path(document, views::duties::pause, &["Uuid"])?;
		contracts.response::<_, DutyView>(document, views::duties::pause, 200, json)?;
		contracts.path(document, views::duties::resume, &["Uuid"])?;
		contracts.response::<_, DutyView>(document, views::duties::resume, 200, json)?;
		contracts.path(document, views::duties::successor, &["Uuid"])?;
		contracts.request::<_, SuccessorInput>(document, views::duties::successor)?;
		contracts.response::<_, DutyView>(document, views::duties::successor, 200, json)?;
		contracts.path(document, views::duties::take_over, &["Uuid"])?;
		contracts.response::<_, DutyView>(document, views::duties::take_over, 200, json)?;
		contracts.path(document, views::duties::run_now, &["Uuid"])?;
		contracts.response::<_, RunNowView>(document, views::duties::run_now, 200, json)?;
		contracts.path(document, views::duties::remove, &["Uuid"])?;
		contracts.response::<_, DeletedView>(document, views::duties::remove, 200, json)?;
	}
	// Duty Triggers and events (#1011).
	{
		use super::duties::{DeletedView, DutyEventReceipt};
		use crate::apps::workspaces::services::duty_events::TriggerView;
		use aidash_domain::duties::events::{DutyEvent, TriggerKind};
		use views::duty_events as view;
		let json = "application/json";
		contracts.path(document, view::list, &["Uuid"])?;
		contracts.response::<_, Vec<TriggerView>>(document, view::list, 200, json)?;
		contracts.path(document, view::create, &["Uuid"])?;
		contracts.request::<_, TriggerKind>(document, view::create)?;
		contracts.response::<_, TriggerView>(document, view::create, 200, json)?;
		contracts.path(document, view::remove, &["Uuid", "Uuid"])?;
		contracts.response::<_, DeletedView>(document, view::remove, 200, json)?;
		contracts.path(document, view::events, &["Uuid"])?;
		contracts.response::<_, Vec<DutyEvent>>(document, view::events, 200, json)?;
		contracts.path(document, view::webhook, &["Uuid"])?;
		contracts.response::<_, DutyEventReceipt>(document, view::webhook, 200, json)?;
		// Another Node's event for a `node_event` trigger (#1013).
		contracts.request::<_, super::duties::NodeEventInput>(document, view::node_event)?;
		contracts.response::<_, DutyEventReceipt>(document, view::node_event, 200, json)?;
	}
	contracts.request::<_, ScratchCreate>(document, views::scratch::create)?;
	contracts.response::<_, ScratchView>(
		document,
		views::scratch::create,
		200,
		"application/json",
	)?;
	contracts.query::<_, ScratchList>(document, views::scratch::limits)?;
	contracts.response::<_, ScratchLimits>(
		document,
		views::scratch::limits,
		200,
		"application/json",
	)?;
	contracts.query::<_, ScratchList>(document, views::scratch::list)?;
	contracts.response::<_, Vec<ScratchView>>(
		document,
		views::scratch::list,
		200,
		"application/json",
	)?;
	contracts.response::<_, ScratchView>(
		document,
		views::scratch::read,
		200,
		"application/json",
	)?;
	contracts.path(document, views::scratch::read, &["Uuid"])?;
	contracts
		.request::<_, super::scratch::ScratchSettingsUpdate>(document, views::scratch::settings)?;
	contracts.request::<_, ScratchMember>(document, views::scratch::add_member)?;
	contracts.request::<_, ScratchMember>(document, views::scratch::remove_member)?;
	contracts.path(document, views::scratch::settings, &["Uuid"])?;
	contracts.response::<_, SentResponse>(
		document,
		views::scratch::settings,
		200,
		"application/json",
	)?;
	contracts.path(document, views::scratch::add_member, &["Uuid"])?;
	contracts.response::<_, SentResponse>(
		document,
		views::scratch::add_member,
		200,
		"application/json",
	)?;
	contracts.path(document, views::scratch::remove_member, &["Uuid"])?;
	contracts.response::<_, SentResponse>(
		document,
		views::scratch::remove_member,
		200,
		"application/json",
	)?;
	contracts.path(document, views::scratch::clear, &["Uuid"])?;
	contracts.request::<_, super::scratch::ScratchClear>(document, views::scratch::clear)?;
	contracts.response::<_, super::scratch::ScratchClearResult>(
		document,
		views::scratch::clear,
		200,
		"application/json",
	)?;
	contracts.path(document, views::scratch::remove, &["Uuid"])?;
	contracts.response::<_, SentResponse>(
		document,
		views::scratch::remove,
		200,
		"application/json",
	)?;
	contracts.response::<_, TaskPage>(
		document,
		views::management::task_list,
		200,
		"application/json",
	)?;
	contracts.query::<_, PageQuery>(document, views::management::task_list)?;
	contracts.response::<_, aidash_domain::threads::ThreadOpened>(
		document,
		views::management::thread_create,
		200,
		"application/json",
	)?;
	contracts.request::<_, ThreadInput>(document, views::management::thread_create)?;
	contracts.response::<_, ThreadSnapshot>(
		document,
		views::management::thread_get,
		200,
		"application/json",
	)?;
	contracts.path(document, views::management::thread_get, &["Uuid"])?;
	contracts.response::<_, Thread>(
		document,
		views::management::thread_update,
		200,
		"application/json",
	)?;
	contracts.request::<_, TitleInput>(document, views::management::thread_update)?;
	contracts.path(document, views::management::thread_update, &["Uuid"])?;
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
	contracts.response::<_, ChannelAttachment>(
		document,
		views::channels::channel_attachment_description,
		200,
		"application/json",
	)?;
	contracts.request::<_, ChannelAttachmentDescription>(
		document,
		views::channels::channel_attachment_description,
	)?;
	contracts.path(
		document,
		views::channels::channel_attachment_description,
		&["Uuid", "Uuid"],
	)?;
	contracts.empty(document, views::channels::channel_attachment_download, 200)?;
	contracts.path(
		document,
		views::channels::channel_attachment_download,
		&["Uuid", "Uuid"],
	)?;
	Ok(())
}
