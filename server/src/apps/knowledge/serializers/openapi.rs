//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::knowledge::serializers::contracts::CleanupStatus as SemanticCleanupStatus;
use crate::apps::knowledge::serializers::contracts::ConfigureIndex as SemanticConfigureIndex;
use crate::apps::knowledge::serializers::contracts::Entry as SemanticEntry;
use crate::apps::knowledge::serializers::contracts::History as SemanticHistory;
use crate::apps::knowledge::serializers::contracts::Index as SemanticIndex;
use crate::apps::knowledge::serializers::contracts::PutEntry as SemanticPutEntry;
use crate::apps::knowledge::serializers::contracts::Revision as SemanticRevision;
use crate::apps::knowledge::serializers::contracts::Search as SemanticSearch;
use crate::apps::knowledge::serializers::contracts::SearchResult as SemanticSearchResult;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, SemanticIndex>(
		document,
		views::entries::configure,
		200,
		"application/json",
	)?;
	contracts.request::<_, SemanticConfigureIndex>(document, views::entries::configure)?;
	contracts.path(document, views::entries::configure, &["Uuid"])?;
	contracts.response::<_, SemanticIndex>(
		document,
		views::entries::index,
		200,
		"application/json",
	)?;
	contracts.path(document, views::entries::index, &["Uuid"])?;
	contracts.response::<_, SemanticEntry>(
		document,
		views::entries::put,
		200,
		"application/json",
	)?;
	contracts.request::<_, SemanticPutEntry>(document, views::entries::put)?;
	contracts.path(document, views::entries::put, &["Uuid"])?;
	contracts.response::<_, Vec<SemanticEntry>>(
		document,
		views::entries::entries,
		200,
		"application/json",
	)?;
	contracts.path(document, views::entries::entries, &["Uuid"])?;
	contracts.response::<_, SemanticEntry>(
		document,
		views::entries::delete,
		200,
		"application/json",
	)?;
	contracts.request::<_, SemanticRevision>(document, views::entries::delete)?;
	contracts.path(document, views::entries::delete, &["Uuid", "Uuid"])?;
	contracts.response::<_, SemanticEntry>(
		document,
		views::entries::reindex,
		200,
		"application/json",
	)?;
	contracts.request::<_, SemanticRevision>(document, views::entries::reindex)?;
	contracts.path(document, views::entries::reindex, &["Uuid", "Uuid"])?;
	contracts.response::<_, SemanticSearchResult>(
		document,
		views::entries::search,
		200,
		"application/json",
	)?;
	contracts.request::<_, SemanticSearch>(document, views::entries::search)?;
	contracts.path(document, views::entries::search, &["Uuid"])?;
	contracts.response::<_, Vec<SemanticHistory>>(
		document,
		views::entries::history,
		200,
		"application/json",
	)?;
	contracts.path(document, views::entries::history, &["Uuid"])?;
	contracts.response::<_, SemanticCleanupStatus>(
		document,
		views::entries::cleanup,
		200,
		"application/json",
	)?;
	contracts.path(document, views::entries::cleanup, &["Uuid"])?;
	contracts.response::<_, Option<crate::semantic::remote::status::Provenance>>(
		document,
		views::provenance::run_receipt,
		200,
		"application/json",
	)?;
	contracts.path(document, views::provenance::run_receipt, &["Uuid"])?;
	Ok(())
}
