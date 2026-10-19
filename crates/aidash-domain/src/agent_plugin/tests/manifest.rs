use super::*;
use rstest::rstest;

fn manifest_with(fields: Value) -> Vec<PluginFile> {
	let mut object = json!({"$schema": PLUGIN_SCHEMA, "name": "demo"});
	for (key, value) in fields.as_object().unwrap() {
		object[key] = value.clone();
	}
	vec![file("plugin.json", &object.to_string())]
}

#[test]
fn every_metadata_field_is_read() {
	let files = manifest_with(json!({
		"version": "1.2.3",
		"description": "Does things",
		"author": {"name": "Ada", "email": "ada@example.com", "url": "https://example.com/ada"},
		"homepage": "https://example.com",
		"repository": "https://github.com/example/demo",
		"license": "Apache-2.0",
		"keywords": ["one", "two"],
	}));
	let plugin = loaded(&files);
	assert_eq!(
		plugin.manifest,
		Manifest {
			name: plugin_name("demo"),
			version: Some("1.2.3".into()),
			description: Some("Does things".into()),
			author: Some(Author {
				name: Some("Ada".into()),
				email: Some("ada@example.com".into()),
				url: Some("https://example.com/ada".into()),
			}),
			homepage: Some("https://example.com".into()),
			repository: Some("https://github.com/example/demo".into()),
			license: Some("Apache-2.0".into()),
			keywords: vec!["one".into(), "two".into()],
		}
	);
	assert_eq!(plugin.findings, []);
}

#[test]
fn only_the_name_and_schema_are_required() {
	let plugin = loaded(&[manifest_file("demo")]);
	assert_eq!(
		plugin.manifest,
		Manifest {
			name: plugin_name("demo"),
			version: None,
			description: None,
			author: None,
			homepage: None,
			repository: None,
			license: None,
			keywords: Vec::new(),
		}
	);
	let author = loaded(&manifest_with(json!({"author": {}})))
		.manifest
		.author;
	assert_eq!(
		author,
		Some(Author {
			name: None,
			email: None,
			url: None
		})
	);
}

#[test]
fn a_missing_manifest_refuses_the_plugin() {
	assert_eq!(load(&[file("README.md", "hi")]), Err(Rejection::NoManifest));
	assert_eq!(
		load(&[file("sub/plugin.json", "{}")]),
		Err(Rejection::NoManifest)
	);
}

#[rstest]
#[case::truncated(b"{\"name\"".as_slice())]
#[case::not_json(b"plugin".as_slice())]
#[case::array(b"[]".as_slice())]
#[case::string(b"\"demo\"".as_slice())]
#[case::null(b"null".as_slice())]
#[case::invalid_utf8(b"{\"name\": \"\xff\"}".as_slice())]
fn unreadable_manifests_are_invalid(#[case] bytes: &[u8]) {
	let files = [PluginFile {
		path: "plugin.json".into(),
		bytes: bytes.to_vec(),
	}];
	assert!(matches!(load(&files), Err(Rejection::InvalidManifest(_))));
}

#[rstest]
#[case::missing(None, Rejection::SchemaViolation("$schema".into()))]
#[case::number(Some(json!(1)), Rejection::SchemaViolation("$schema".into()))]
#[case::future_version(
	Some(json!("https://agent-plugins.org/schemas/2.0.0/plugin.schema.json")),
	Rejection::UnsupportedVersion("2.0.0".into())
)]
#[case::draft_version(
	Some(json!("https://agent-plugins.org/schemas/1.0.0-rc.1/plugin.schema.json")),
	Rejection::UnsupportedVersion("1.0.0-rc.1".into())
)]
#[case::other_host(
	Some(json!("https://example.com/schemas/1.0.0/plugin.schema.json")),
	Rejection::SchemaViolation("$schema".into())
)]
#[case::wrong_document(
	Some(json!("https://agent-plugins.org/schemas/1.0.0/mcp.schema.json")),
	Rejection::SchemaViolation("$schema".into())
)]
#[case::empty_version(
	Some(json!("https://agent-plugins.org/schemas//plugin.schema.json")),
	Rejection::SchemaViolation("$schema".into())
)]
#[case::nested_version(
	Some(json!("https://agent-plugins.org/schemas/1/0/plugin.schema.json")),
	Rejection::SchemaViolation("$schema".into())
)]
fn the_schema_decides_whether_the_version_is_supported(
	#[case] schema: Option<Value>,
	#[case] expected: Rejection,
) {
	let mut object = json!({"name": "demo"});
	if let Some(schema) = schema {
		object["$schema"] = schema;
	}
	let files = [file("plugin.json", &object.to_string())];
	assert_eq!(load(&files), Err(expected));
}

#[rstest]
#[case::leading_hyphen("-a")]
#[case::trailing_hyphen("a-")]
#[case::double_hyphen("a--b")]
#[case::double_dot("a..b")]
#[case::leading_dot(".a")]
#[case::trailing_dot("a.")]
#[case::uppercase("A")]
#[case::underscore("a_b")]
#[case::space("a b")]
#[case::unicode("café")]
#[case::empty("")]
#[case::too_long(&"a".repeat(65))]
fn invalid_names_refuse_the_plugin(#[case] name: &str) {
	assert_eq!(
		load(&[manifest_file(name)]),
		Err(Rejection::SchemaViolation("name".into()))
	);
}

#[rstest]
#[case::single("a")]
#[case::digits("0")]
#[case::hyphenated("my-plugin")]
#[case::dotted("my.plugin")]
#[case::dot_then_hyphen("a.-b")]
#[case::longest(&"a".repeat(64))]
fn valid_names_load(#[case] name: &str) {
	assert_eq!(loaded(&[manifest_file(name)]).manifest.name.as_str(), name);
}

#[test]
fn a_missing_or_non_string_name_refuses_the_plugin() {
	for object in [
		json!({"$schema": PLUGIN_SCHEMA}),
		json!({"$schema": PLUGIN_SCHEMA, "name": 7}),
	] {
		let files = [file("plugin.json", &object.to_string())];
		assert_eq!(load(&files), Err(Rejection::SchemaViolation("name".into())));
	}
}

#[rstest]
#[case::version("version", json!(1), "version")]
#[case::description("description", json!(["x"]), "description")]
#[case::homepage("homepage", json!(true), "homepage")]
#[case::repository("repository", json!({}), "repository")]
#[case::license("license", json!(null), "license")]
#[case::keywords_string("keywords", json!("one"), "keywords")]
#[case::keywords_item("keywords", json!(["one", 2]), "keywords")]
#[case::author_string("author", json!("Ada"), "author")]
#[case::author_name("author", json!({"name": 1}), "author.name")]
#[case::author_email("author", json!({"email": []}), "author.email")]
#[case::author_unknown_key("author", json!({"name": "Ada", "twitter": "@ada"}), "author.twitter")]
fn wrong_metadata_shapes_refuse_the_plugin(
	#[case] field: &str,
	#[case] value: Value,
	#[case] violation: &str,
) {
	let files = manifest_with(json!({ field: value }));
	assert_eq!(
		load(&files),
		Err(Rejection::SchemaViolation(violation.into()))
	);
}

#[rstest]
#[case::number(json!(3))]
#[case::null(json!(null))]
#[case::array(json!([{"com.example": {}}]))]
#[case::string(json!("com.example"))]
fn an_extensions_value_that_is_not_an_object_is_ignored_and_reported(#[case] extensions: Value) {
	let plugin = loaded(&manifest_with(json!({ "extensions": extensions })));
	assert_eq!(
		plugin.findings,
		[Finding::ignored(
			Component::Manifest,
			Reason::ExtensionsNotObject,
			"extensions"
		)]
	);
}

#[test]
fn extensions_members_must_be_objects() {
	let files =
		manifest_with(json!({"extensions": {"com.example": {"any": [1, 2]}, "org.other": 1}}));
	assert_eq!(
		load(&files),
		Err(Rejection::SchemaViolation("extensions".into()))
	);
}

#[test]
fn valid_extensions_are_accepted_without_a_finding() {
	let files =
		manifest_with(json!({"extensions": {"com.example": {"any": [1, 2]}, "org.other": {}}}));
	assert_eq!(loaded(&files).findings, []);
}

#[test]
fn unknown_top_level_fields_are_ignored_and_reported_in_name_order() {
	let files = manifest_with(json!({"zeta": 1, "alpha": {"x": 1}, "Skills": []}));
	let plugin = loaded(&files);
	assert_eq!(
		plugin.findings,
		["Skills", "alpha", "zeta"].map(|field| Finding::ignored(
			Component::Manifest,
			Reason::UnknownField,
			field
		))
	);
	assert_eq!(plugin.manifest.name.as_str(), "demo");
}
