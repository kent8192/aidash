//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::operations::serializers::contracts::DeploymentStatus;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, DeploymentStatus>(
		document,
		views::deployment::status,
		200,
		"application/json",
	)?;
	contracts.empty(document, views::deployment::status, 503)?;
	Ok(())
}
