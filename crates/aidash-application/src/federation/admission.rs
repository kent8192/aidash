//! Receiver inspection must name exactly the approved direct dependencies.
use crate::{Error, Result};
use aidash_domain::{
	federation::execution::Inspection,
	registry::{EntityRef, Search, rules::digest},
};
use std::collections::BTreeMap;

pub fn validate_inspection(
	validation: &crate::registry::DefinitionValidation,
	inspection: &Inspection,
	node: &str,
	agent: &EntityRef,
	requirements: &Search,
) -> Result<()> {
	if inspection.authority_digest.len() != 71
		|| !inspection.authority_digest.starts_with("sha256:")
		|| !inspection.authority_digest.as_bytes()[7..]
			.iter()
			.all(u8::is_ascii_hexdigit)
		|| inspection.node_id != node
		|| inspection.agent.id != agent.id
		|| inspection.agent.version != agent.version
		|| inspection.agent.kind != "agent"
		|| !requirements.matches(&inspection.agent)
	{
		return Err(Error::External(
			"invalid receiver execution inspection".into(),
		));
	}
	validation.validate_in(&inspection.agent, true)?;
	let snapshot = &inspection.binding_snapshot;
	snapshot.validate()?;
	if !snapshot.remote || snapshot.agent.registry_node != node || snapshot.agent.local() != *agent
	{
		return Err(Error::External(
			"receiver Binding placement mismatch".into(),
		));
	}
	let mut expected = BTreeMap::new();
	for pinned in &snapshot.definitions {
		if pinned.identity.registry_node != node {
			return Err(Error::External(
				"unsupported foreign receiver dependency".into(),
			));
		}
		expected.insert(
			(&pinned.identity.id, &pinned.identity.version),
			pinned.definition.kind.as_str(),
		);
	}
	if let Some(reference) = &inspection.compactor {
		expected.insert((&reference.id, &reference.version), "compactor");
	}
	if inspection.definitions.len() != expected.len() {
		return Err(Error::External("incomplete receiver definitions".into()));
	}
	for definition in &inspection.definitions {
		if definition.metadata.id != definition.entry.id
			|| definition.metadata.version != definition.entry.version
			|| definition.metadata.kind != definition.kind
			|| definition.digest != digest(&serde_json::to_value(&definition.metadata)?)
		{
			return Err(Error::External(
				"receiver definition metadata mismatch".into(),
			));
		}
		if let Some(pinned) = snapshot
			.definitions
			.iter()
			.find(|p| p.identity.local() == definition.entry)
			&& (pinned.digest != definition.digest
				|| serde_json::to_value(&pinned.definition)?
					!= serde_json::to_value(&definition.metadata)?)
		{
			return Err(Error::External(
				"receiver snapshot definition mismatch".into(),
			));
		}

		// Dependencies execute on the receiver. Do not resolve that node's
		// credential environment or executable paths on this source node.
		aidash_domain::policy::identifier(&definition.entry.id)?;
		semver::Version::parse(&definition.entry.version)
			.map_err(|_| Error::External("invalid receiver version".into()))?;
		if expected.remove(&(&definition.entry.id, &definition.entry.version))
			!= Some(definition.kind.as_str())
			|| definition.digest.len() != 71
			|| !definition.digest.starts_with("sha256:")
			|| !definition.digest[7..]
				.bytes()
				.all(|b| b.is_ascii_hexdigit())
		{
			return Err(Error::External("invalid receiver definition".into()));
		}
		if definition.kind == "agent"
			&& definition.digest != digest(&serde_json::to_value(&inspection.agent)?)
		{
			return Err(Error::External("receiver agent digest mismatch".into()));
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests;
