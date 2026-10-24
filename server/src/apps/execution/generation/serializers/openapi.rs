//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::execution::generation::serializers::contracts::Assignment as GenerationAssignment;
use crate::apps::execution::generation::serializers::contracts::Request as GenerationRequest;
use crate::apps::execution::generation::serializers::lifecycle::Control as GenerationControl;
use crate::apps::execution::generation::serializers::lifecycle::History as GenerationHistory;
use crate::apps::execution::generation::serializers::policy::Policy as GenerationPolicy;
use crate::apps::execution::generation::serializers::policy::Spec as GenerationSpec;
use crate::apps::execution::generation::serializers::requests::AssignInput as GenerationAssignInput;
use crate::apps::execution::generation::serializers::requests::PolicyUpdate as GenerationPolicyUpdate;
use crate::apps::execution::generation::serializers::requests::Usage as GenerationUsage;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, GenerationPolicy>(
		document,
		views::requests::set_policy,
		200,
		"application/json",
	)?;
	contracts.request::<_, GenerationPolicyUpdate>(document, views::requests::set_policy)?;
	contracts.path(document, views::requests::set_policy, &["String", "String"])?;
	contracts.response::<_, Vec<GenerationPolicy>>(
		document,
		views::requests::policies,
		200,
		"application/json",
	)?;
	contracts.path(document, views::requests::policies, &["String"])?;
	contracts.response::<_, GenerationAssignment>(
		document,
		views::requests::assign,
		200,
		"application/json",
	)?;
	contracts.request::<_, GenerationAssignInput>(document, views::requests::assign)?;
	contracts.path(document, views::requests::assign, &["String", "Uuid"])?;
	contracts.response::<_, Vec<GenerationRequest>>(
		document,
		views::requests::requests,
		200,
		"application/json",
	)?;
	contracts.path(document, views::requests::requests, &["String"])?;
	contracts.response::<_, GenerationRequest>(
		document,
		views::requests::control,
		200,
		"application/json",
	)?;
	contracts.request::<_, GenerationControl>(document, views::requests::control)?;
	contracts.path(document, views::requests::control, &["String", "Uuid"])?;
	contracts.response::<_, Vec<GenerationHistory>>(
		document,
		views::requests::history,
		200,
		"application/json",
	)?;
	contracts.path(document, views::requests::history, &["String", "Uuid"])?;
	contracts.response::<_, GenerationUsage>(
		document,
		views::requests::usage,
		200,
		"application/json",
	)?;
	contracts.path(document, views::requests::usage, &["String", "Uuid"])?;
	contracts.response::<_, GenerationSpec>(
		document,
		views::requests::spec,
		200,
		"application/json",
	)?;
	contracts.path(document, views::requests::spec, &["String", "Uuid"])?;
	contracts.response::<_, crate::generation::foreign::Prepared>(
		document,
		views::foreign::request,
		200,
		"application/json",
	)?;
	contracts.request::<_, crate::generation::foreign::Input>(document, views::foreign::request)?;
	contracts.path(document, views::foreign::request, &["Uuid"])?;
	contracts.response::<_, bool>(document, views::foreign::cancel, 200, "application/json")?;
	contracts.path(document, views::foreign::cancel, &["Uuid", "Uuid"])?;
	Ok(())
}
