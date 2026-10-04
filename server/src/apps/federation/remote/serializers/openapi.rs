//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::federation::remote::serializers::management::DelegateInput;
use crate::apps::federation::remote::serializers::management::RemoteActionInput;
use crate::apps::federation::remote::serializers::runtime::Delegation;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;
use serde_json::Value;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, Delegation>(
		document,
		views::management::task_delegate,
		200,
		"application/json",
	)?;
	contracts.request::<_, DelegateInput>(document, views::management::task_delegate)?;
	contracts.path(document, views::management::task_delegate, &["Uuid"])?;
	contracts.response::<_, Value>(
		document,
		views::management::remote_action,
		200,
		"application/json",
	)?;
	contracts.request::<_, RemoteActionInput>(document, views::management::remote_action)?;
	Ok(())
}
