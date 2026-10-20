//! Deployment and Node identity (ADR 0028, ADR 0064). A Deployment is one
//! installation, `aidash://<name>`. A Node is an agent hub inside one Tenant,
//! addressed as `aidash://<deployment>/<node name>`. Nothing else in the
//! workspace parses or formats these forms.
use crate::identity::tenancy::{TenancyRules, assigned_name};
use crate::tenancy::TenantName;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

const SCHEME: &str = "aidash://";
const DEPLOYMENT_NAME_MAX: usize = 100;

/// `aidash://<name>`: 1 to 100 ASCII letters, digits or hyphens after the scheme.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeploymentId(String);

impl DeploymentId {
	pub const MAX_LEN: usize = SCHEME.len() + DEPLOYMENT_NAME_MAX;

	pub fn parse(value: &str) -> Result<Self> {
		let name = value.strip_prefix(SCHEME).unwrap_or_default();
		if name.is_empty()
			|| name.len() > DEPLOYMENT_NAME_MAX
			|| !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
		{
			return Err(Error::Invalid(
				"a Deployment id is aidash:// followed by letters, numbers or hyphens".into(),
			));
		}
		Ok(Self(value.to_owned()))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	pub fn into_string(self) -> String {
		self.0
	}

	/// The part after `aidash://`.
	pub fn name(&self) -> &str {
		&self.0[SCHEME.len()..]
	}

	/// The address of `node` in this Deployment.
	pub fn node(&self, node: NodeName) -> NodeAddress {
		NodeAddress {
			deployment: self.clone(),
			node,
		}
	}
}

impl fmt::Display for DeploymentId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl FromStr for DeploymentId {
	type Err = Error;
	fn from_str(value: &str) -> Result<Self> {
		Self::parse(value)
	}
}

impl TryFrom<String> for DeploymentId {
	type Error = Error;
	fn try_from(value: String) -> Result<Self> {
		Self::parse(&value)
	}
}

impl From<DeploymentId> for String {
	fn from(value: DeploymentId) -> Self {
		value.0
	}
}

impl AsRef<str> for DeploymentId {
	fn as_ref(&self) -> &str {
		&self.0
	}
}

impl PartialEq<str> for DeploymentId {
	fn eq(&self, other: &str) -> bool {
		self.0 == other
	}
}

impl PartialEq<&str> for DeploymentId {
	fn eq(&self, other: &&str) -> bool {
		self.0 == *other
	}
}

impl PartialEq<String> for DeploymentId {
	fn eq(&self, other: &String) -> bool {
		&self.0 == other
	}
}

impl PartialEq<DeploymentId> for String {
	fn eq(&self, other: &DeploymentId) -> bool {
		self == &other.0
	}
}

/// Lets tests compare a JSON field with the Deployment id directly.
impl PartialEq<DeploymentId> for serde_json::Value {
	fn eq(&self, other: &DeploymentId) -> bool {
		self.as_str() == Some(other.0.as_str())
	}
}

impl PartialEq<DeploymentId> for &str {
	fn eq(&self, other: &DeploymentId) -> bool {
		*self == other.0
	}
}

impl PartialEq<DeploymentId> for str {
	fn eq(&self, other: &DeploymentId) -> bool {
		self == other.0
	}
}

/// The identifier Aidash assigns to a Node, in the same form as a Tenant name:
/// `<adjective>-<noun>-<8 random characters>`. Unique within its Deployment,
/// never changed and never reused.
#[derive(
	Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct NodeName(String);

impl NodeName {
	pub const MAX_LEN: usize = crate::identity::tenancy::ASSIGNED_NAME_MAX_LEN;

	/// Draws a name from 16 bytes of caller-supplied entropy, such as a random
	/// UUID, so this crate stays free of a random source.
	pub fn from_entropy(entropy: [u8; 16]) -> Self {
		Self(assigned_name(entropy))
	}

	/// A stable name for `seed`, for fixtures that must name a Node before it
	/// exists. Production Nodes draw theirs from entropy.
	pub fn derived(seed: &str) -> Self {
		use sha2::{Digest, Sha256};
		let digest = Sha256::digest(seed.as_bytes());
		Self::from_entropy(digest[..16].try_into().expect("16 bytes"))
	}

	/// Accepts the assigned form only: three lowercase segments, the last one
	/// eight letters or digits.
	pub fn parse(value: &str) -> Result<Self> {
		let invalid = || Error::Invalid("a Node name is <adjective>-<noun>-<8 characters>".into());
		if value.len() > Self::MAX_LEN {
			return Err(invalid());
		}
		let mut parts = value.split('-');
		let (Some(adjective), Some(noun), Some(suffix), None) =
			(parts.next(), parts.next(), parts.next(), parts.next())
		else {
			return Err(invalid());
		};
		let word = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase());
		if !word(adjective)
			|| !word(noun)
			|| suffix.len() != crate::identity::tenancy::RANDOM_LENGTH
			|| !suffix
				.bytes()
				.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
		{
			return Err(invalid());
		}
		Ok(Self(value.to_owned()))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	pub fn into_string(self) -> String {
		self.0
	}
}

impl fmt::Display for NodeName {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl FromStr for NodeName {
	type Err = Error;
	fn from_str(value: &str) -> Result<Self> {
		Self::parse(value)
	}
}

impl TryFrom<String> for NodeName {
	type Error = Error;
	fn try_from(value: String) -> Result<Self> {
		Self::parse(&value)
	}
}

impl From<NodeName> for String {
	fn from(value: NodeName) -> Self {
		value.0
	}
}

impl AsRef<str> for NodeName {
	fn as_ref(&self) -> &str {
		&self.0
	}
}

impl PartialEq<str> for NodeName {
	fn eq(&self, other: &str) -> bool {
		self.0 == other
	}
}

impl PartialEq<&str> for NodeName {
	fn eq(&self, other: &&str) -> bool {
		self.0 == *other
	}
}

impl PartialEq<String> for NodeName {
	fn eq(&self, other: &String) -> bool {
		&self.0 == other
	}
}

/// `aidash://<deployment>/<node>`. The Tenant is never part of it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NodeAddress {
	pub deployment: DeploymentId,
	pub node: NodeName,
}

impl NodeAddress {
	pub const MAX_LEN: usize = DeploymentId::MAX_LEN + 1 + NodeName::MAX_LEN;

	/// Accepts exactly one `/` after the authority and refuses a trailing path.
	pub fn parse(value: &str) -> Result<Self> {
		let invalid =
			|| Error::Invalid("a Node address is aidash://<deployment>/<node name>".into());
		if value.len() > Self::MAX_LEN {
			return Err(invalid());
		}
		let rest = value.strip_prefix(SCHEME).ok_or_else(invalid)?;
		let (deployment, node) = rest.split_once('/').ok_or_else(invalid)?;
		Ok(Self {
			deployment: DeploymentId::parse(&format!("{SCHEME}{deployment}"))
				.map_err(|_| invalid())?,
			node: NodeName::parse(node).map_err(|_| invalid())?,
		})
	}
}

impl NodeAddress {
	/// The address of the Node `NodeName::derived(seed)` names in `deployment`,
	/// for fixtures.
	pub fn derived(deployment: &str, seed: &str) -> Self {
		Self {
			deployment: DeploymentId::parse(deployment).expect("a Deployment id"),
			node: NodeName::derived(seed),
		}
	}
}

impl fmt::Display for NodeAddress {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}/{}", self.deployment, self.node)
	}
}

impl FromStr for NodeAddress {
	type Err = Error;
	fn from_str(value: &str) -> Result<Self> {
		Self::parse(value)
	}
}

impl TryFrom<String> for NodeAddress {
	type Error = Error;
	fn try_from(value: String) -> Result<Self> {
		Self::parse(&value)
	}
}

impl From<NodeAddress> for String {
	fn from(value: NodeAddress) -> Self {
		value.to_string()
	}
}

/// The JWS `kid` of a Deployment Key: base64url without padding of SHA-256
/// over the 32 raw public-key bytes, 43 characters. It is what a Tenant
/// Administrator types when registering a Node and what `/.well-known/aidash`
/// publishes. The glossary's Fingerprint is a different thing, a Key Material
/// digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeploymentKeyId(String);

impl DeploymentKeyId {
	pub const LEN: usize = 43;

	pub fn of_public_key(public_key: &[u8; 32]) -> Self {
		use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
		use sha2::{Digest, Sha256};
		Self(URL_SAFE_NO_PAD.encode(Sha256::digest(public_key)))
	}

	pub fn parse(value: &str) -> Result<Self> {
		if value.len() != Self::LEN
			|| !value
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
		{
			return Err(Error::Invalid(
				"a key ID is 43 base64url characters without padding".into(),
			));
		}
		Ok(Self(value.to_owned()))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl fmt::Display for DeploymentKeyId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl TryFrom<String> for DeploymentKeyId {
	type Error = Error;
	fn try_from(value: String) -> Result<Self> {
		Self::parse(&value)
	}
}

impl From<DeploymentKeyId> for String {
	fn from(value: DeploymentKeyId) -> Self {
		value.0
	}
}

/// The Deployment part of a Node address. A bare Deployment id, which a peer
/// that predates Node addresses still sends, is its own Deployment.
pub fn deployment_of(address: &str) -> &str {
	let rest = address.strip_prefix(SCHEME).unwrap_or(address);
	let end = rest.find('/').unwrap_or(rest.len());
	&address[..address.len() - rest.len() + end]
}

/// A Node as a peer names it: a Node address, or a bare Deployment id from a
/// peer that predates Node addresses.
pub fn validate_node_ref(value: &str) -> Result<()> {
	if value
		.strip_prefix(SCHEME)
		.is_some_and(|rest| rest.contains('/'))
	{
		NodeAddress::parse(value).map(drop)
	} else {
		DeploymentId::parse(value).map(drop)
	}
}

/// Whether `reference` names the Node at `home`: by its address, or by the
/// bare Deployment id a Tool transport still carries.
pub fn names_home(reference: &str, home: &str) -> bool {
	reference == home || reference == deployment_of(home)
}

/// Whether `home` is the address of `node` in `deployment`, without building it.
pub fn is_home(home: &str, deployment: &str, node: &str) -> bool {
	home.strip_prefix(deployment)
		.and_then(|rest| rest.strip_prefix('/'))
		== Some(node)
}

/// A Node as its Tenant sees it. Its name is assigned; its label is free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
	pub tenant: TenantName,
	pub name: NodeName,
	pub label: String,
	/// The Node every Tenant starts with. Exactly one per Tenant.
	pub default: bool,
	pub created_at: DateTime<Utc>,
}

/// The label of the Node every Tenant starts with.
pub const DEFAULT_LABEL: &str = "Default";

/// A Node label as stored, with the Tenant label's rule: trimmed, 1 to 64
/// characters, without control characters.
pub fn label(value: &str) -> Result<String> {
	crate::identity::tenancy::label(value).map_err(|_| {
		Error::Invalid("a Node label needs 1 to 64 characters without control characters".into())
	})
}

/// How many Nodes a Tenant may hold: the Tenancy Rule, narrowed by the Tier's
/// limit when the Tier sets one.
pub fn node_limit(rules: &TenancyRules, tier: Option<u32>) -> u32 {
	tier.map_or(rules.nodes_per_tenant, |tier| {
		tier.min(rules.nodes_per_tenant)
	})
}

/// Why a Node cannot be created now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeLimitReached;

/// Refuses a creation that would exceed `limit`, given `count` Nodes today.
pub fn check_creation(limit: u32, count: u32) -> std::result::Result<(), NodeLimitReached> {
	if count >= limit {
		return Err(NodeLimitReached);
	}
	Ok(())
}

pub mod registration;

#[cfg(test)]
mod tests;
