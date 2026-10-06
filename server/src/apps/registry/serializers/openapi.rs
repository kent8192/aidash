//! OpenAPI payload contracts for native endpoints.
use super::super::views;
use crate::apps::registry::serializers::contracts::Entry as RegistryContractsEntry;
use crate::apps::registry::serializers::contracts::Package;
use crate::apps::registry::serializers::contracts::PackageRecord;
use crate::apps::registry::serializers::contracts::Search;
use crate::apps::registry::serializers::knowledge::PersonalAgent;
use crate::apps::registry::serializers::management::InstallInput;
use crate::apps::registry::serializers::skill_import::ImportRequest;
use crate::apps::registry::serializers::skill_import::ImportResult;
use crate::{Result, config::openapi::Contracts};
use reinhardt::rest::openapi::OpenApiSchema;

pub(crate) fn register(contracts: &mut Contracts, document: &mut OpenApiSchema) -> Result<()> {
	contracts.named::<aidash_domain::registry::Localized>(document, "BTreeMap")?;
	contracts.response::<_, Vec<RegistryContractsEntry>>(
		document,
		views::management::registry_list,
		200,
		"application/json",
	)?;
	contracts.query::<_, Search>(document, views::management::registry_list)?;
	contracts.response::<_, RegistryContractsEntry>(
		document,
		views::management::registry_get,
		200,
		"application/json",
	)?;
	contracts.path(
		document,
		views::management::registry_get,
		&["String", "String"],
	)?;
	contracts.response::<_, RegistryContractsEntry>(
		document,
		views::management::registry_create,
		200,
		"application/json",
	)?;
	contracts.request::<_, RegistryContractsEntry>(document, views::management::registry_create)?;
	contracts.idempotency(document, views::management::registry_create)?;
	contracts.response::<_, ImportResult>(
		document,
		views::management::skill_import,
		200,
		"application/json",
	)?;
	contracts.request::<_, ImportRequest>(document, views::management::skill_import)?;
	contracts.response::<_, Vec<PackageRecord>>(
		document,
		views::management::marketplace,
		200,
		"application/json",
	)?;
	contracts.query::<_, Search>(document, views::management::marketplace)?;
	contracts.response::<_, PackageRecord>(
		document,
		views::management::package_publish,
		200,
		"application/json",
	)?;
	contracts.request::<_, Package>(document, views::management::package_publish)?;
	contracts.response::<_, RegistryContractsEntry>(
		document,
		views::management::package_install,
		200,
		"application/json",
	)?;
	contracts.request::<_, InstallInput>(document, views::management::package_install)?;
	contracts.path(
		document,
		views::management::package_install,
		&["String", "String"],
	)?;
	contracts.response::<_, RegistryContractsEntry>(
		document,
		views::personal_agents::create,
		200,
		"application/json",
	)?;
	contracts.request::<_, PersonalAgent>(document, views::personal_agents::create)?;
	Ok(())
}
