//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::federation::transactions::serializers::contracts::LocalStatus as TransactionParticipantStatus;
use crate::apps::federation::transactions::serializers::contracts::Manifest as TransactionManifest;
use crate::apps::federation::transactions::serializers::contracts::Status as AtomicTransaction;
use crate::apps::federation::transactions::serializers::protocol::TransactionDetails;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, AtomicTransaction>(
		document,
		views::protocol::submit,
		202,
		"application/json",
	)?;
	contracts.request::<_, TransactionManifest>(document, views::protocol::submit)?;
	contracts.response::<_, Vec<AtomicTransaction>>(
		document,
		views::protocol::list,
		200,
		"application/json",
	)?;
	contracts.response::<_, TransactionDetails>(
		document,
		views::protocol::details,
		200,
		"application/json",
	)?;
	contracts.path(document, views::protocol::details, &["Uuid"])?;
	contracts.response::<_, AtomicTransaction>(
		document,
		views::protocol::abort,
		200,
		"application/json",
	)?;
	contracts.path(document, views::protocol::abort, &["Uuid"])?;
	contracts.response::<_, Vec<TransactionParticipantStatus>>(
		document,
		views::protocol::participants,
		200,
		"application/json",
	)?;
	Ok(())
}
