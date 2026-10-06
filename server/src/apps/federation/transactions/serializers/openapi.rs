//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::federation::transactions::serializers::contracts::LocalStatus as TransactionParticipantStatus;
use crate::apps::federation::transactions::serializers::contracts::Manifest as TransactionManifest;
use crate::apps::federation::transactions::serializers::contracts::Status as AtomicTransaction;
use crate::apps::federation::transactions::serializers::protocol::TransactionDetails;
use crate::apps::federation::transactions::serializers::protocol::TransactionTrust;
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
	contracts.response::<_, Vec<super::protocol::TrustChange>>(
		document,
		views::protocol::trust_list,
		200,
		"application/json",
	)?;
	contracts.response::<_, super::protocol::TrustChange>(
		document,
		views::protocol::trust,
		200,
		"application/json",
	)?;
	contracts.request::<_, TransactionTrust>(document, views::protocol::trust)?;
	contracts.response::<_, super::protocol::TrustChange>(
		document,
		views::protocol::trust,
		202,
		"application/json",
	)?;
	contracts.response::<_, crate::federation::Peer>(
		document,
		views::protocol::restore_peer,
		200,
		"application/json",
	)?;
	contracts
		.request::<_, super::protocol::PeerRecovery>(document, views::protocol::restore_peer)?;
	Ok(())
}
