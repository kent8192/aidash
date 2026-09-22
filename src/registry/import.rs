//! Atomic imports reuse the same validation and immutable-version contract as registration.
use super::{AgentConfig, ClusterConfig, EntityRef, Entry, validate};
use crate::{Error, Result, federation::Federation, tool::ToolConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryImport {
	pub entries: Vec<Entry>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub(crate) struct ImportResult {
	pub imported: usize,
	pub unchanged: usize,
}

fn dependencies(entry: &Entry, node_id: &str) -> Result<Vec<EntityRef>> {
	Ok(match entry.kind.as_str() {
		"agent" => {
			let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
			std::iter::once(config.model)
				.chain(config.tools)
				.chain(config.skills)
				.chain(config.cluster)
				.collect()
		}
		"cluster" => {
			let config: ClusterConfig = serde_json::from_value(entry.config.clone())?;
			vec![config.coordinator]
		}
		"tool" => match serde_json::from_value(entry.config.clone())? {
			ToolConfig::Agent {
				node_id: target,
				agent,
			} if target == node_id => vec![agent],
			_ => vec![],
		},
		_ => vec![],
	})
}

fn ordered(entries: &[Entry], node_id: &str) -> Result<Vec<usize>> {
	if entries.is_empty() || entries.len() > 100 {
		return Err(Error::Invalid("import requires 1 to 100 entries".into()));
	}
	let mut identities = BTreeMap::new();
	for (index, entry) in entries.iter().enumerate() {
		validate(entry)
			.map_err(|error| Error::Invalid(format!("{}@{}: {error}", entry.id, entry.version)))?;
		if identities
			.insert((&entry.id, &entry.version), index)
			.is_some()
		{
			return Err(Error::Invalid(format!(
				"duplicate import entry {}@{}",
				entry.id, entry.version
			)));
		}
	}
	let dependencies = entries
		.iter()
		.map(|entry| {
			Ok(dependencies(entry, node_id)?
				.iter()
				.filter_map(|reference| {
					identities
						.get(&(&reference.id, &reference.version))
						.copied()
				})
				.collect::<Vec<_>>())
		})
		.collect::<Result<Vec<_>>>()?;
	let mut order = Vec::with_capacity(entries.len());
	let mut included = vec![false; entries.len()];
	while order.len() < entries.len() {
		let mut ready = dependencies
			.iter()
			.enumerate()
			.filter(|(index, dependencies)| {
				!included[*index] && dependencies.iter().all(|dependency| included[*dependency])
			})
			.map(|(index, _)| index)
			.collect::<Vec<_>>();
		if ready.is_empty() {
			return Err(Error::Invalid(
				"import contains cyclic entity references".into(),
			));
		}
		ready.sort_unstable_by(|left, right| {
			entries[*left]
				.id
				.cmp(&entries[*right].id)
				.then_with(|| entries[*left].version.cmp(&entries[*right].version))
		});
		for index in ready {
			included[index] = true;
			order.push(index);
		}
	}
	Ok(order)
}

pub(crate) async fn import(f: &Federation, input: RegistryImport) -> Result<ImportResult> {
	let order = ordered(&input.entries, &f.config.node_id)?;
	let mut tx = f.store.pool.begin().await?;
	let mut imported = 0;
	for index in order {
		let entry = &input.entries[index];
		let inserted = super::register_in(&mut tx, entry, &f.config.node_id)
			.await
			.map_err(|error| match error {
				Error::Conflict(message) => {
					Error::Conflict(format!("{}@{}: {message}", entry.id, entry.version))
				}
				Error::Invalid(message) | Error::NotFound(message) => {
					Error::Invalid(format!("{}@{}: {message}", entry.id, entry.version))
				}
				other => other,
			})?;
		if inserted {
			f.store
				.event(
					&mut tx,
					None,
					"registry.registered",
					json!({"id":entry.id,"version":entry.version,"kind":entry.kind}),
				)
				.await?;
			imported += 1;
		}
	}
	tx.commit().await?;
	Ok(ImportResult {
		imported,
		unchanged: input.entries.len() - imported,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn skill(id: &str, version: &str) -> Entry {
		serde_json::from_value(json!({
			"id": id,
			"version": version,
			"kind": "skill",
			"name": {"en": id},
			"description": {"en": "import fixture"},
			"config": {"instructions": "Read carefully."}
		}))
		.unwrap()
	}

	#[test]
	fn orders_independent_ready_entries_by_identity_not_input_order() {
		let reversed = vec![
			skill("zeta", "1.0.0"),
			skill("alpha", "2.0.0"),
			skill("alpha", "1.0.0"),
		];
		let forward = vec![
			skill("alpha", "1.0.0"),
			skill("alpha", "2.0.0"),
			skill("zeta", "1.0.0"),
		];
		let ids = |entries: &[Entry]| {
			ordered(entries, "node")
				.unwrap()
				.into_iter()
				.map(|index| (entries[index].id.clone(), entries[index].version.clone()))
				.collect::<Vec<_>>()
		};

		let expected = vec![
			("alpha".to_owned(), "1.0.0".to_owned()),
			("alpha".to_owned(), "2.0.0".to_owned()),
			("zeta".to_owned(), "1.0.0".to_owned()),
		];
		assert_eq!(ids(&reversed), expected);
		assert_eq!(ids(&forward), expected);
	}
}
