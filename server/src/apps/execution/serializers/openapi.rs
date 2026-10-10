//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::execution::serializers::human_requests::HumanRequest;
use crate::apps::execution::serializers::management::ClaimInput;
use crate::apps::execution::serializers::management::ControlInput;
use crate::apps::execution::serializers::management::EventQuery;
use crate::apps::execution::serializers::management::InferenceStreamQuery;
use crate::apps::execution::serializers::openrouter::CatalogModel;
use crate::apps::execution::serializers::runs::RunDetails;
use crate::apps::execution::serializers::state::StateResponse;
use crate::apps::federation::remote::serializers::actions::SentResponse;
use crate::apps::workspaces::serializers::entities::Event;
use crate::apps::workspaces::serializers::entities::Run;
use crate::apps::workspaces::serializers::entities::Task;
use crate::apps::workspaces::serializers::management::MessageInput;
use crate::apps::workspaces::serializers::tasks::PageQuery;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;
use serde_json::Value;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, StateResponse>(
		document,
		views::management::state,
		200,
		"application/json",
	)?;
	contracts.response::<_, Vec<CatalogModel>>(
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
	contracts.response::<_, Run>(
		document,
		views::management::run_control,
		200,
		"application/json",
	)?;
	contracts.request::<_, ControlInput>(document, views::management::run_control)?;
	contracts.path(document, views::management::run_control, &["Uuid"])?;
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
	contracts.response::<_, Vec<Event>>(
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
