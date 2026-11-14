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
	/// `scope` is the Carried Approval's own scope, which may be narrower
	/// than its source's when the carry did not succeed on every Node.
	async fn write_carried(
		&mut self,
		node: &NodeAddress,
		source: &Source,
		entry: &Entry,
		scope: ApprovalScope,
	) -> Result<()>;
}

/// Carry from the highest eligible lower version independently on each Node.
///
/// Every Node is decided before anything is written. A Deployment-scoped
/// source that carries on only some of its Nodes gives those Nodes
/// Node-scoped rows, so the Deployment-scoped rows of one version never
/// differ between Nodes.
pub async fn carry(scope: &mut dyn CarryScope, entry: &Entry) -> Result<Vec<CarryResult>> {
	let mut decided = Vec::new();
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
			decided.push((
				None,
				CarryResult {
					node,
					source_version: None,
					changed: vec!["no earlier carrying approval".into()],
					unapproved_dependencies: vec![],
					carried: false,
				},
			));
			continue;
		};
		let changed = match carry_over::compare(&source.definition, entry) {
			Ok(changed) => changed,
			Err(error) => vec![format!("consent comparison failed: {error}")],
		};
		let mut missing = Vec::new();
		for dependency in approvals::dependencies(entry, node.deployment.as_str())? {
			if !crate::registry::system::builtin_reference(&dependency)
				&& !scope.approved(&node, &dependency).await?
			{
				missing.push(dependency);
			}
		}
		let carried = changed.is_empty() && missing.is_empty();
		decided.push((
			Some(source.clone()),
			CarryResult {
				node,
				source_version: Some(source.definition.version.clone()),
				changed,
				unapproved_dependencies: missing,
				carried,
			},
		));
	}
	let partial = decided.iter().any(|(source, result)| {
		source
			.as_ref()
			.is_some_and(|source| source.scope == ApprovalScope::Deployment && !result.carried)
	});
	let mut results = Vec::new();
	for (source, result) in decided {
		if let Some(source) = source.filter(|_| result.carried) {
			let scope_of_copy = if source.scope == ApprovalScope::Deployment && !partial {
				ApprovalScope::Deployment
			} else {
				ApprovalScope::Node
			};
			scope
				.write_carried(&result.node, &source, entry, scope_of_copy)
				.await?;
		}
		results.push(result);
	}
	Ok(results)
}

#[cfg(test)]
mod tests;
