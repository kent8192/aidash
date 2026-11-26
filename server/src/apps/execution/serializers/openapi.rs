//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::execution::serializers::human_requests::HumanRequest;
use crate::apps::execution::serializers::management::ClaimInput;
use crate::apps::execution::serializers::management::EventPage;
use crate::apps::execution::serializers::management::EventQuery;
use crate::apps::execution::serializers::management::InferenceStreamQuery;
use crate::apps::execution::serializers::management::{
	ControlInput, DeclineCallInput, SuspendedRuns, SuspendedRunsResume,
};
use crate::apps::execution::serializers::runs::RunDetails;
use crate::apps::execution::serializers::state::{
	StateQuery, StateResponse, ThreadPage, ThreadPageQuery,
};
use crate::apps::federation::remote::serializers::actions::SentResponse;
use crate::apps::workspaces::serializers::entities::Task;
use crate::apps::workspaces::serializers::management::MessageInput;
use crate::apps::workspaces::serializers::tasks::PageQuery;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;
use serde_json::Value;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, Vec<crate::apps::execution::serializers::subnodes::SubnodeView>>(
		document,
		views::management::run_subnodes,
		200,
		"application/json",
	)?;
	contracts.path(document, views::management::run_subnodes, &["Uuid"])?;
	contracts.response::<_, crate::apps::execution::serializers::subnodes::Dissolved>(
		document,
		views::management::subnode_dissolve,
		200,
		"application/json",
	)?;
	contracts.path(document, views::management::subnode_dissolve, &["Uuid"])?;

	contracts.response::<_, aidash_application::authorization::bounded_reads::RunPage>(
		document,
		views::management::task_runs,
		200,
		"application/json",
	)?;
	contracts
		.query::<_, super::management::RunListQuery>(document, views::management::task_runs)?;
	contracts.path(document, views::management::task_runs, &["Uuid"])?;
	contracts.response::<_, aidash_application::authorization::bounded_reads::RunPage>(
		document,
		views::management::thread_runs,
		200,
		"application/json",
	)?;
	contracts.query::<_, super::management::ThreadRunListQuery>(
		document,
		views::management::thread_runs,
	)?;
	contracts.path(document, views::management::thread_runs, &["Uuid"])?;
	contracts.response::<_, aidash_application::authorization::bounded_reads::HumanPage>(
		document,
		views::management::human_requests,
		200,
		"application/json",
	)?;
	contracts.query::<_, super::management::HumanListQuery>(
		document,
		views::management::human_requests,
	)?;
	contracts.response::<_, StateResponse>(
		document,
		views::management::state,
		200,
		"application/json",
	)?;
	contracts.query::<_, StateQuery>(document, views::management::state)?;
	contracts.response::<_, ThreadPage>(
		document,
		crate::apps::workspaces::views::management::thread_list,
		200,
		"application/json",
	)?;
	contracts.query::<_, ThreadPageQuery>(
		document,
		crate::apps::workspaces::views::management::thread_list,
	)?;
	contracts.response::<_, aidash_domain::catalog::CatalogSnapshot>(
		document,
		views::management::openrouter_models,
		200,
		"application/json",
	)?;
	contracts.response::<_, Task>(
		document,
		views::management::task_claim,
		200,
		"application/json",
	)?;
	contracts.request::<_, ClaimInput>(document, views::management::task_claim)?;
	contracts.path(document, views::management::task_claim, &["Uuid"])?;
	contracts.response::<_, RunDetails>(
		document,
		views::management::run_get,
		200,
		"application/json",
	)?;
	contracts.query::<_, PageQuery>(document, views::management::run_get)?;
	contracts.path(document, views::management::run_get, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::identity::services::execution::management::RunManagement>(
			document,
			views::management::run_control,
			200,
			"application/json",
		)?;
	contracts.request::<_, ControlInput>(document, views::management::run_control)?;
	contracts.path(document, views::management::run_control, &["Uuid"])?;
	contracts
		.response::<_, crate::apps::identity::services::execution::management::RunManagement>(
			document,
			views::management::run_decline_call,
			200,
			"application/json",
		)?;
	contracts.request::<_, DeclineCallInput>(document, views::management::run_decline_call)?;
	contracts.response::<_, SuspendedRuns>(
		document,
		views::management::suspended_runs,
		200,
		"application/json",
	)?;
	contracts.response::<_, SuspendedRunsResume>(
		document,
		views::management::suspended_runs_resume,
		200,
		"application/json",
	)?;
	contracts.path(document, views::management::run_decline_call, &["Uuid"])?;
	contracts.response::<_, SentResponse>(
		document,
		views::management::run_message,
		200,
		"application/json",
	)?;
	contracts.empty(document, views::management::run_message, 409)?;
	contracts.request::<_, MessageInput>(document, views::management::run_message)?;
	contracts.path(document, views::management::run_message, &["Uuid"])?;
	contracts.response::<_, HumanRequest>(
		document,
		views::management::human_answer,
		200,
		"application/json",
	)?;
	contracts.request::<_, Value>(document, views::management::human_answer)?;
	contracts.path(document, views::management::human_answer, &["Uuid"])?;
	contracts.response::<_, EventPage>(
		document,
		views::management::events,
		200,
		"application/json",
	)?;
	contracts.query::<_, EventQuery>(document, views::management::events)?;
	contracts.response::<_, String>(
		document,
		views::management::stream,
		200,
		"text/event-stream",
	)?;
	contracts.query::<_, EventQuery>(document, views::management::stream)?;
	contracts.response::<_, String>(
		document,
		views::management::run_inference_stream,
		200,
		"text/event-stream",
	)?;
	contracts
		.query::<_, InferenceStreamQuery>(document, views::management::run_inference_stream)?;
	contracts.path(document, views::management::run_inference_stream, &["Uuid"])?;
	Ok(())
}
