//! Receiver inspection must name exactly the approved direct dependencies.
use crate::{Error, Result};
use aidash_domain::{
	federation::execution::Inspection,
	registry::{AgentConfig, EntityRef, Search, rules::digest},
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
	let config: AgentConfig = serde_json::from_value(inspection.agent.config.clone())?;
	let mut expected = BTreeMap::new();
	for (reference, kind) in std::iter::once((agent, "agent"))
		.chain(std::iter::once((&config.model, "model")))
		.chain(config.tools.iter().map(|r| (r, "tool")))
		.chain(config.skills.iter().map(|r| (r, "skill")))
		.chain(config.cluster.iter().map(|r| (r, "cluster")))
		.chain(inspection.compactor.iter().map(|r| (r, "compactor")))
	{
		if let Some(previous) = expected.insert((&reference.id, &reference.version), kind)
			&& previous != kind
		{
			return Err(Error::External(
				"inconsistent receiver dependency kinds".into(),
			));
		}
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
