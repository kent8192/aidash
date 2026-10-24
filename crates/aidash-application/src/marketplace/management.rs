//! Bounded discovery and disclosure under current distribution and catalog authority.
use super::{definitions, distribution, installations};
use crate::{Error, Result, ports::marketplace::MarketplaceRead};
use aidash_domain::{
	marketplace::definitions::key,
	marketplace::{Audience, Installation, InstallationRevision, Summary},
	registry::Entry,
};
use serde_json::json;

pub struct BrowseQuery {
	pub q: String,
	pub offset: usize,
	pub limit: usize,
}
pub struct SourceQuery {
	pub offset: usize,
	pub limit: usize,
}
pub struct SourcePage {
	pub entries: Vec<Entry>,
	pub next_offset: Option<usize>,
}

pub async fn browse(
	scope: &mut dyn MarketplaceRead,
	input: &BrowseQuery,
	node: &str,
) -> Result<Vec<Summary>> {
	scope.operation(
		"marketplace.browse",
		json!({"query":input.q,"offset":input.offset,"limit":input.limit}),
		None,
	);
	let mut visible = vec![];
	let mut skipped = 0;
	let mut cursor = String::new();
	let search_query = input.q.to_lowercase();
	loop {
		let candidates = scope.version_page(&cursor, 64).await?;
		let exhausted = candidates.len() < 64;
		for (key, candidate) in candidates {
			cursor = key;
			let result = async {
				scope
					.require(&scope.package_resource(&candidate), "marketplace.browse")
					.await?;
				let tenant = scope.tenant().to_owned();
				distribution::distributed(scope, &candidate, &tenant).await?;
				distribution::summary(scope, &candidate, node).await
			}
			.await;
			let summary = match result {
				Ok(s) => s,
				Err(Error::Forbidden) => continue,
				Err(e) => return Err(e),
			};
			// Search only public summary fields, never configuration/dependency IDs.
			let search = json!([
				summary.name,
				summary.description,
				summary.package_id,
				summary.author,
				summary.kind,
				summary.capabilities,
				summary.languages
			])
			.to_string()
			.to_lowercase();
			if !search.contains(&search_query) {
				continue;
			}
			if skipped < input.offset {
				skipped += 1;
				continue;
			}
			visible.push(summary);
			if visible.len() == input.limit {
				break;
			}
		}
		if visible.len() == input.limit || exhausted {
			break;
		}
	}
	Ok(visible)
}
pub async fn sources(
	scope: &mut dyn MarketplaceRead,
	input: &SourceQuery,
	node: &str,
) -> Result<SourcePage> {
	scope.operation(
		"marketplace.sources",
		json!({"offset":input.offset,"limit":input.limit}),
		None,
	);
	let mut cursor = input.offset;
	let mut more = false;
	let mut sources = vec![];
	// Denied candidates consume the same bounded scan budget as readable entries.
	while cursor - input.offset < 256 {
		let candidates = scope.source_references(cursor, 64).await?;
		let count = candidates.len();
		more = count == 64;
		for (index, reference) in candidates.into_iter().enumerate() {
			cursor += 1;
			let readable = async {
				let entry = scope.catalog(&reference, "registry.read").await?;
				if !installations::active(scope, &entry).await? {
					return Err(Error::Forbidden);
				}
				installations::check_pinned(scope, &entry).await?;
				scope.require_export(&entry).await?;
				definitions::private_context(scope, &entry).await?;
				definitions::publication_graph(scope, &entry, &[], node).await?;
				Ok(entry)
			}
			.await;
			match readable {
				Ok(entry) => sources.push(entry),
				Err(Error::Forbidden | Error::NotFound(_)) => {}
				Err(error) => return Err(error),
			}
			if sources.len() == input.limit {
				more = index + 1 < count || more;
				return Ok(SourcePage {
					entries: sources,
					next_offset: more.then_some(cursor),
				});
			}
		}
		if !more {
			break;
		}
	}
	Ok(SourcePage {
		entries: sources,
		next_offset: more.then_some(cursor),
	})
}
pub async fn read_consent(
	scope: &mut dyn MarketplaceRead,
	package: &str,
	tenant: &str,
) -> Result<Audience> {
	aidash_domain::policy::identifier(tenant)?;
	scope.operation(
		"marketplace.redistribution.manage",
		json!({"key":package,"redistributor":tenant}),
		None,
	);
	let version = scope.version(package).await?.ok_or(Error::Forbidden)?;
	if version.owner_tenant != scope.tenant() {
		return Err(Error::Forbidden);
	}
	scope
		.require(
			&scope.package_resource(&version),
			"marketplace.redistribution.manage",
		)
		.await?;
	let consent_key = key(&(package, tenant));
	let current = scope.consent(&consent_key).await?.unwrap_or(Audience {
		revision: 0,
		tenants: Default::default(),
	});
	scope.authority(
		format!("marketplace_consents:{consent_key}"),
		current.revision,
	);
	Ok(current)
}
pub async fn list_installations(
	scope: &mut dyn MarketplaceRead,
	node: &str,
) -> Result<Vec<InstallationRevision>> {
	let documents = scope.installation_documents().await?;
	let mut visible = vec![];
	for document in documents {
		let install: Installation = serde_json::from_value(document)?;
		match installations::view(scope, &install.id, None, node).await {
			Ok(view) => visible.push(view),
			Err(Error::Forbidden) => {}
			Err(e) => return Err(e),
		}
	}
	Ok(visible)
}
#[cfg(test)]
mod tests;
