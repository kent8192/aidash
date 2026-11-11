//! Carry approvals during registration under the caller's transaction and policy lock.
use crate::{Result, authorization::catalog::approvals};
use aidash_domain::{
	catalog::carry_over::{self, CarryCandidate},
	identity::catalog::ApprovalScope,
	nodes::NodeAddress,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;

/// An eligible source's immutable definition and the original approver.
#[derive(Clone, Debug)]
pub struct Source {
	pub definition: Entry,
	pub enabled: bool,
	pub carry_over: bool,
	pub scope: ApprovalScope,
	pub approver: String,
}

/// Each result tells the registrant why a Node did or did not carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarryResult {
	pub node: NodeAddress,
	pub source_version: Option<String>,
	pub changed: Vec<String>,
	pub unapproved_dependencies: Vec<EntityRef>,
	pub carried: bool,
}

/// The implementation owns one transaction and must persist carried rows,
/// policy membership and restriction revisions atomically with registration.
#[async_trait]
pub trait CarryScope: Send {
	async fn nodes(&mut self) -> Result<Vec<NodeAddress>>;
	async fn sources(&mut self, node: &NodeAddress, id: &str) -> Result<Vec<Source>>;
	async fn approved(&mut self, node: &NodeAddress, dependency: &EntityRef) -> Result<bool>;
	async fn write_carried(
		&mut self,
		node: &NodeAddress,
		source: &Source,
		entry: &Entry,
	) -> Result<()>;
}

/// Carry from the highest eligible lower version independently on each Node.
pub async fn carry(scope: &mut dyn CarryScope, entry: &Entry) -> Result<Vec<CarryResult>> {
	let mut results = Vec::new();
	for node in scope.nodes().await? {
		let sources = scope.sources(&node, &entry.id).await?;
		if sources
			.iter()
			.any(|source| source.definition.id != entry.id || source.definition.kind != entry.kind)
		{
			return Err(crate::Error::Invalid(
				"carry source identity differs from registration".into(),
			));
		}
		let candidates: Vec<_> = sources
			.iter()
			.map(|source| CarryCandidate {
				version: &source.definition.version,
				enabled: source.enabled,
				carry_over: source.carry_over,
			})
			.collect();
		let selected = carry_over::source_version(&entry.version, &candidates)?;
		let Some(source) = selected.and_then(|version| {
			sources.iter().find(|source| {
				source.definition.version == version && source.enabled && source.carry_over
			})
		}) else {
			results.push(CarryResult {
				node,
				source_version: None,
				changed: vec!["no earlier carrying approval".into()],
				unapproved_dependencies: vec![],
				carried: false,
			});
			continue;
		};
		let changed = carry_over::compare(&source.definition, entry)?;
		let mut missing = Vec::new();
		for dependency in approvals::dependencies(entry, node.deployment.as_str())? {
			if !crate::registry::system::builtin_reference(&dependency)
				&& !scope.approved(&node, &dependency).await?
			{
				missing.push(dependency);
			}
		}
		let carried = changed.is_empty() && missing.is_empty();
		if carried {
			scope.write_carried(&node, source, entry).await?;
		}
		results.push(CarryResult {
			node,
			source_version: Some(source.definition.version.clone()),
			changed,
			unapproved_dependencies: missing,
			carried,
		});
	}
	Ok(results)
}

#[cfg(test)]
mod tests;
