//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::federation::peer::serializers::mesh::MeshResponse;
use crate::apps::federation::remote::serializers::runtime::Discovery;
use crate::apps::federation::remote::serializers::runtime::Peer;
use crate::apps::registry::serializers::contracts::Search;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, Peer>(
		document,
		views::management::peer_create,
		200,
		"application/json",
	)?;
	contracts.request::<_, Peer>(document, views::management::peer_create)?;
	contracts.response::<_, Discovery>(
		document,
		views::management::discover,
		200,
		"application/json",
	)?;
	contracts.request::<_, Search>(document, views::management::discover)?;
	contracts.response::<_, MeshResponse>(
		document,
		views::management::mesh,
		200,
		"application/json",
	)?;
	Ok(())
}
