//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::federation::remote::serializers::runtime::Discovery;
use crate::apps::registry::serializers::contracts::Search;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, Discovery>(
		document,
		views::management::discover,
		200,
		"application/json",
	)?;
	contracts.request::<_, Search>(document, views::management::discover)?;
	contracts.query::<_, views::management::SendingNode>(document, views::management::discover)?;
	Ok(())
}
