//! Native endpoint contracts for Marketplace.
use super::super::views;
use super::{contracts::*, management::*};
use crate::{Result, config::openapi::Contracts, registry::Entry};
use reinhardt::rest::openapi::OpenApiSchema;
pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.response::<_, Vec<Summary>>(
		document,
		views::management::browse,
		200,
		"application/json",
	)?;
	contracts.query::<_, Browse>(document, views::management::browse)?;
	contracts.response::<_, Detail>(
		document,
		views::management::detail,
		200,
		"application/json",
	)?;
	contracts.path(document, views::management::detail, &["String"])?;
	contracts.response::<_, serde_json::Value>(
		document,
		views::management::publish,
		200,
		"application/json",
	)?;
	contracts.request::<_, Publish>(document, views::management::publish)?;
	contracts.response::<_, Vec<Entry>>(
		document,
		views::management::sources,
		200,
		"application/json",
	)?;
	contracts.query::<_, SourceQuery>(document, views::management::sources)?;
	contracts.response::<_, InstallationRevision>(
		document,
		views::management::install,
		200,
		"application/json",
	)?;
	contracts.request::<_, Install>(document, views::management::install)?;
	contracts.path(document, views::management::install, &["String"])?;
	contracts.response::<_, Audience>(
		document,
		views::management::share,
		200,
		"application/json",
	)?;
	contracts.request::<_, AudienceInput>(document, views::management::share)?;
	contracts.path(document, views::management::share, &["String"])?;
	contracts.response::<_, Audience>(
		document,
		views::management::consent,
		200,
		"application/json",
	)?;
	contracts.request::<_, AudienceInput>(document, views::management::consent)?;
	contracts.path(document, views::management::consent, &["String", "String"])?;
	contracts.response::<_, Audience>(
		document,
		views::management::read_consent,
		200,
		"application/json",
	)?;
	contracts.path(
		document,
		views::management::read_consent,
		&["String", "String"],
	)?;
	contracts.response::<_, Vec<InstallationRevision>>(
		document,
		views::management::list_installations,
		200,
		"application/json",
	)?;
	contracts.response::<_, InstallationRevision>(
		document,
		views::management::installation,
		200,
		"application/json",
	)?;
	contracts.query::<_, RevisionQuery>(document, views::management::installation)?;
	contracts.path(document, views::management::installation, &["String"])?;
	contracts.response::<_, InstallationRevision>(
		document,
		views::management::configure,
		200,
		"application/json",
	)?;
	contracts.request::<_, Configure>(document, views::management::configure)?;
	contracts.path(document, views::management::configure, &["String"])?;
	contracts.response::<_, Compatibility>(
		document,
		views::management::compatibility,
		200,
		"application/json",
	)?;
	contracts.response::<_, Compatibility>(
		document,
		views::management::set_compatibility,
		200,
		"application/json",
	)?;
	contracts.request::<_, CompatibilityInput>(document, views::management::set_compatibility)?;
	contracts.response::<_, Installation>(
		document,
		views::management::activate,
		200,
		"application/json",
	)?;
	contracts.request::<_, Activate>(document, views::management::activate)?;
	contracts.path(document, views::management::activate, &["String"])?;
	contracts.response::<_, Installation>(
		document,
		views::management::adopt,
		200,
		"application/json",
	)?;
	contracts.request::<_, Adopt>(document, views::management::adopt)?;
	contracts.response::<_, Vec<InstallationRevision>>(
		document,
		views::management::administration,
		200,
		"application/json",
	)?;
	contracts.query::<_, AdministrationQuery>(document, views::management::administration)?;
	contracts.response::<_, PublicationPreview>(
		document,
		views::management::publication_access,
		200,
		"application/json",
	)?;
	contracts.request::<_, Publish>(document, views::management::publication_access)?;
	contracts.response_header::<_, usize>(
		document,
		views::management::sources,
		200,
		"x-aidash-next-offset",
		"Next candidate offset; may accompany an empty page",
	)?;
	contracts.response_header::<_, usize>(
		document,
		views::management::administration,
		200,
		"x-aidash-next-offset",
		"Next revision offset",
	)?;
	Ok(())
}
