//! `plugin.json`: the one file whose violations refuse the whole plugin.
use super::{
	Component, Finding, PLUGIN_SCHEMA, PluginFile, Reason, Rejection, SUPPORTED_VERSION,
	schema_version,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

const MANIFEST_PATH: &str = "plugin.json";
const MAX_NAME_LEN: usize = 64;

pub(super) const TOP_LEVEL_KEYS: [&str; 10] = [
	"$schema",
	"name",
	"version",
	"description",
	"author",
	"homepage",
	"repository",
	"license",
	"keywords",
	"extensions",
];
pub(super) const AUTHOR_KEYS: [&str; 3] = ["name", "email", "url"];

/// A plugin name: 1-64 characters of `a-z0-9.-`, starting and ending alphanumeric, with neither
/// `--` nor `..`.
#[derive(
	Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct PluginName(String);

impl PluginName {
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

fn valid_plugin_name(name: &str) -> bool {
	let alphanumeric = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
	!name.is_empty()
		&& name.len() <= MAX_NAME_LEN
		&& name.starts_with(alphanumeric)
		&& name.ends_with(alphanumeric)
		&& name
			.chars()
			.all(|c| alphanumeric(c) || c == '-' || c == '.')
		&& !name.contains("--")
		&& !name.contains("..")
}

impl TryFrom<String> for PluginName {
	type Error = crate::Error;

	fn try_from(name: String) -> Result<Self, Self::Error> {
		if valid_plugin_name(&name) {
			Ok(Self(name))
		} else {
			Err(crate::Error::Invalid("INVALID_PLUGIN_NAME".into()))
		}
	}
}

impl From<PluginName> for String {
	fn from(name: PluginName) -> Self {
		name.0
	}
}

impl fmt::Display for PluginName {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

/// Formats of the metadata strings are not checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Manifest {
	pub name: PluginName,
	pub version: Option<String>,
	pub description: Option<String>,
	pub author: Option<Author>,
	pub homepage: Option<String>,
	pub repository: Option<String>,
	pub license: Option<String>,
	#[serde(default)]
	pub keywords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Author {
	pub name: Option<String>,
	pub email: Option<String>,
	pub url: Option<String>,
}

pub(super) fn parse(files: &[PluginFile]) -> Result<(Manifest, Vec<Finding>), Rejection> {
	let file = files
		.iter()
		.find(|file| file.path == MANIFEST_PATH)
		.ok_or(Rejection::NoManifest)?;
	let value: Value = serde_json::from_slice(&file.bytes)
		.map_err(|error| Rejection::InvalidManifest(error.to_string()))?;
	let Value::Object(object) = value else {
		return Err(Rejection::InvalidManifest("not a JSON object".into()));
	};
	check_schema(&object)?;
	let name = match object.get("name") {
		Some(Value::String(name)) => PluginName::try_from(name.clone())
			.map_err(|_| Rejection::SchemaViolation("name".into()))?,
		_ => return Err(Rejection::SchemaViolation("name".into())),
	};
	let manifest = Manifest {
		name,
		version: optional_string(&object, "version")?,
		description: optional_string(&object, "description")?,
		author: author(&object)?,
		homepage: optional_string(&object, "homepage")?,
		repository: optional_string(&object, "repository")?,
		license: optional_string(&object, "license")?,
		keywords: keywords(&object)?,
	};
	let mut findings = Vec::new();
	if let Some(extensions) = object.get("extensions") {
		match extensions {
			Value::Object(namespaces) if namespaces.values().all(Value::is_object) => {}
			Value::Object(_) => return Err(Rejection::SchemaViolation("extensions".into())),
			_ => findings.push(Finding::ignored(
				Component::Manifest,
				Reason::ExtensionsNotObject,
				"extensions",
			)),
		}
	}
	let mut unknown: Vec<&String> = object
		.keys()
		.filter(|key| !TOP_LEVEL_KEYS.contains(&key.as_str()))
		.collect();
	unknown.sort();
	findings.extend(
		unknown
			.into_iter()
			.map(|key| Finding::ignored(Component::Manifest, Reason::UnknownField, key.as_str())),
	);
	Ok((manifest, findings))
}

fn check_schema(object: &Map<String, Value>) -> Result<(), Rejection> {
	let schema = match object.get("$schema") {
		Some(Value::String(schema)) => schema,
		_ => return Err(Rejection::SchemaViolation("$schema".into())),
	};
	match schema_version(schema, "plugin.schema.json") {
		Some(SUPPORTED_VERSION) => {
			debug_assert_eq!(schema, PLUGIN_SCHEMA);
			Ok(())
		}
		Some(version) => Err(Rejection::UnsupportedVersion(version.to_owned())),
		None => Err(Rejection::SchemaViolation("$schema".into())),
	}
}

fn optional_string(object: &Map<String, Value>, field: &str) -> Result<Option<String>, Rejection> {
	match object.get(field) {
		None => Ok(None),
		Some(Value::String(text)) => Ok(Some(text.clone())),
		Some(_) => Err(Rejection::SchemaViolation(field.into())),
	}
}

fn keywords(object: &Map<String, Value>) -> Result<Vec<String>, Rejection> {
	match object.get("keywords") {
		None => Ok(Vec::new()),
		Some(Value::Array(items)) => items
			.iter()
			.map(|item| match item {
				Value::String(text) => Ok(text.clone()),
				_ => Err(Rejection::SchemaViolation("keywords".into())),
			})
			.collect(),
		Some(_) => Err(Rejection::SchemaViolation("keywords".into())),
	}
}

fn author(object: &Map<String, Value>) -> Result<Option<Author>, Rejection> {
	let author = match object.get("author") {
		None => return Ok(None),
		Some(Value::Object(author)) => author,
		Some(_) => return Err(Rejection::SchemaViolation("author".into())),
	};
	if let Some(key) = author
		.keys()
		.find(|key| !AUTHOR_KEYS.contains(&key.as_str()))
	{
		return Err(Rejection::SchemaViolation(format!("author.{key}")));
	}
	let member = |key: &str| match author.get(key) {
		None => Ok(None),
		Some(Value::String(text)) => Ok(Some(text.clone())),
		Some(_) => Err(Rejection::SchemaViolation(format!("author.{key}"))),
	};
	Ok(Some(Author {
		name: member("name")?,
		email: member("email")?,
		url: member("url")?,
	}))
}
