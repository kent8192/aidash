//! The two inputs of the `demo-data` command (ADR 0080): the structure the
//! repository checks in, and the people a secret supplies. Both are parsed
//! strictly and validated before anything touches the database, and a digest
//! of the pair tells a rerun whether anything changed.
use crate::{Error, Result};
use aidash_domain::identity::tenancy::{ADMINISTRATORS, MEMBERS};
use aidash_domain::registry::Entry;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// The longest an email address may be (RFC 5321).
const EMAIL_MAX: usize = 320;
/// The longest a structure key may be.
const KEY_MAX: usize = 32;

/// `structure.json`: fictional Tenants and what they hold.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Structure {
	pub tenants: Vec<StructureTenant>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructureTenant {
	/// The stable identifier of this Tenant inside Demo data, never shown.
	pub key: String,
	/// The Tenant label.
	pub label: String,
	/// Fictional user subjects. They never sign in.
	#[serde(default)]
	pub subjects: Vec<StructureSubject>,
	/// The Tenant's Registry entries, dependencies first. A string in an
	/// entry's `config` that is exactly `$node` stands for the Deployment's
	/// node id, which Tool descriptors and binding targets must name.
	#[serde(default)]
	pub registry: Vec<Entry>,
	/// Catalog Approvals to grant on the Deployment's Node.
	#[serde(default)]
	pub approvals: Vec<StructureApproval>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructureSubject {
	pub id: String,
	pub groups: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructureApproval {
	pub entry: ApprovedEntry,
	pub groups: BTreeSet<String>,
}

/// One Registry entry version a Catalog Approval names.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedEntry {
	pub id: String,
	pub version: String,
}

/// `people.json`: who may claim which fictional subject. It lives in a secret,
/// so a fork and a run without `--people` simply have none.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct People {
	pub people: Vec<Person>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Person {
	/// The verified email address; validation lowercases it.
	pub email: String,
	/// A structure key.
	pub tenant: String,
	pub subject: String,
	pub role: Role,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
	Administrator,
	Member,
}

impl Role {
	/// The Assignable Group the role puts a subject in.
	pub fn group(self) -> &'static str {
		match self {
			Self::Administrator => ADMINISTRATORS,
			Self::Member => MEMBERS,
		}
	}
}

/// Both inputs, validated, with the digest that names them.
#[derive(Debug)]
pub struct Inputs {
	pub structure: Structure,
	pub people: Option<People>,
	/// Hex SHA-256 of the canonical JSON of both inputs.
	pub digest: String,
}

impl Inputs {
	/// Parses and validates `structure` and, when given, `people`. The error
	/// names the input that was refused.
	pub fn parse(structure: &str, people: Option<&str>) -> Result<Self> {
		let parsed: Structure = serde_json::from_str(structure)
			.map_err(|error| Error::Invalid(format!("structure: {error}")))?;
		let mut listed: Option<People> = people
			.map(serde_json::from_str)
			.transpose()
			.map_err(|error| Error::Invalid(format!("people: {error}")))?;
		validate_structure(&parsed)?;
		if let Some(listed) = &mut listed {
			validate_people(listed, &parsed)?;
		}
		let digest = digest(structure, people)?;
		Ok(Self {
			structure: parsed,
			people: listed,
			digest,
		})
	}

	/// The People who name `tenant`, in input order.
	pub fn people_of<'a>(&'a self, tenant: &'a str) -> impl Iterator<Item = &'a Person> {
		self.people
			.iter()
			.flat_map(|people| people.people.iter())
			.filter(move |person| person.tenant == tenant)
	}
}

/// Whether `key` is a structure key: `[a-z][a-z0-9-]{0,31}`.
pub fn valid_key(key: &str) -> bool {
	let mut characters = key.chars();
	characters.next().is_some_and(|c| c.is_ascii_lowercase())
		&& key.len() <= KEY_MAX
		&& characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn validate_structure(structure: &Structure) -> Result<()> {
	let mut keys = BTreeSet::new();
	for tenant in &structure.tenants {
		if !valid_key(&tenant.key) {
			return Err(Error::Invalid(format!(
				"structure: tenant key {:?} must match [a-z][a-z0-9-]{{0,31}}",
				tenant.key
			)));
		}
		if !keys.insert(tenant.key.as_str()) {
			return Err(Error::Invalid(format!(
				"structure: tenant key {:?} appears twice",
				tenant.key
			)));
		}
		aidash_domain::identity::tenancy::label(&tenant.label).map_err(|error| {
			Error::Invalid(format!("structure: tenant {}: {error}", tenant.key))
		})?;
		validate_tenant(tenant)?;
	}
	Ok(())
}

fn validate_tenant(tenant: &StructureTenant) -> Result<()> {
	let key = &tenant.key;
	let mut subjects = BTreeSet::new();
	for subject in &tenant.subjects {
		aidash_domain::policy::identifier(&subject.id).map_err(|error| {
			Error::Invalid(format!("structure: tenant {key}: subject: {error}"))
		})?;
		if !subjects.insert(subject.id.as_str()) {
			return Err(Error::Invalid(format!(
				"structure: tenant {key}: subject {:?} appears twice",
				subject.id
			)));
		}
		assignable(&subject.groups).map_err(|error| {
			Error::Invalid(format!(
				"structure: tenant {key}: subject {}: {error}",
				subject.id
			))
		})?;
	}
	let mut entries = BTreeSet::new();
	for entry in &tenant.registry {
		if !entries.insert((entry.id.as_str(), entry.version.as_str())) {
			return Err(Error::Invalid(format!(
				"structure: tenant {key}: registry entry {}@{} appears twice",
				entry.id, entry.version
			)));
		}
	}
	let mut approved = BTreeSet::new();
	for approval in &tenant.approvals {
		if !approved.insert((approval.entry.id.as_str(), approval.entry.version.as_str())) {
			return Err(Error::Invalid(format!(
				"structure: tenant {key}: approval of {}@{} appears twice",
				approval.entry.id, approval.entry.version
			)));
		}
		if approval.groups.is_empty() {
			return Err(Error::Invalid(format!(
				"structure: tenant {key}: approval of {}@{} names no group",
				approval.entry.id, approval.entry.version
			)));
		}
		assignable(&approval.groups).map_err(|error| {
			Error::Invalid(format!(
				"structure: tenant {key}: approval of {}@{}: {error}",
				approval.entry.id, approval.entry.version
			))
		})?;
	}
	Ok(())
}

/// Demo data assigns only the two Assignable Groups every Tenant starts with.
fn assignable(groups: &BTreeSet<String>) -> std::result::Result<(), String> {
	match groups
		.iter()
		.find(|group| ![ADMINISTRATORS, MEMBERS].contains(&group.as_str()))
	{
		Some(group) => Err(format!(
			"group {group:?} must be {ADMINISTRATORS:?} or {MEMBERS:?}"
		)),
		None => Ok(()),
	}
}

fn validate_people(people: &mut People, structure: &Structure) -> Result<()> {
	let keys: BTreeSet<_> = structure
		.tenants
		.iter()
		.map(|tenant| tenant.key.as_str())
		.collect();
	let mut seen = BTreeSet::new();
	for person in &mut people.people {
		if !keys.contains(person.tenant.as_str()) {
			return Err(Error::Invalid(format!(
				"people: tenant {:?} is not a structure key",
				person.tenant
			)));
		}
		aidash_domain::policy::identifier(&person.subject)
			.map_err(|error| Error::Invalid(format!("people: subject: {error}")))?;
		person.email = email(&person.email)?;
		if !seen.insert((person.tenant.clone(), person.email.clone())) {
			return Err(Error::Invalid(format!(
				"people: {} appears twice for tenant {}",
				person.email, person.tenant
			)));
		}
	}
	Ok(())
}

/// The lowercased address, or an error when it cannot be one. Addresses are
/// matched against a verified email, so only the shape is checked here.
fn email(value: &str) -> Result<String> {
	let address = value.trim().to_lowercase();
	let shaped = address.len() <= EMAIL_MAX
		&& !address.chars().any(|c| c.is_whitespace() || c.is_control())
		&& address.split_once('@').is_some_and(|(local, domain)| {
			!local.is_empty() && !domain.is_empty() && !domain.contains('@')
		});
	if !shaped {
		return Err(Error::Invalid(
			"people: an email must have one @ with text on both sides, at most 320 bytes".into(),
		));
	}
	Ok(address)
}

/// The digest of both inputs: SHA-256, in hex, of the canonical JSON of
/// `{"structure": ..., "people": ...}`, with `people` left out when absent.
/// Canonical JSON sorts every object's keys and has no insignificant
/// whitespace, so neither the order of keys nor the layout of a file changes it.
fn digest(structure: &str, people: Option<&str>) -> Result<String> {
	let value = |text: &str, input: &str| {
		serde_json::from_str::<Value>(text)
			.map_err(|error| Error::Invalid(format!("{input}: {error}")))
	};
	let mut document = json!({"structure": value(structure, "structure")?});
	if let Some(people) = people {
		document["people"] = value(people, "people")?;
	}
	Ok(format!("{:x}", Sha256::digest(canonical(&document))))
}

/// `value` as canonical JSON.
fn canonical(value: &Value) -> String {
	match value {
		Value::Object(object) => {
			let sorted: BTreeMap<_, _> = object.iter().collect();
			let members: Vec<_> = sorted
				.into_iter()
				.map(|(key, value)| format!("{}:{}", Value::String(key.clone()), canonical(value)))
				.collect();
			format!("{{{}}}", members.join(","))
		}
		Value::Array(items) => {
			let items: Vec<_> = items.iter().map(canonical).collect();
			format!("[{}]", items.join(","))
		}
		scalar => scalar.to_string(),
	}
}

#[cfg(test)]
mod tests;
