//! Reinhardt endpoint discovery with serde-complete application contracts.
//!
//! Schemars preserves the recursive policy expressions and distinct serde input
//! and output contracts. Reinhardt owns endpoint discovery, the OpenAPI document,
//! security, and response/parameter builders.
use crate::{Error, Result, apps};
use reinhardt::core::endpoint::EndpointInfo;
use reinhardt::rest::openapi::endpoint_inspector::InspectorConfig;
use reinhardt::rest::openapi::openapi::{
	Header, Http, HttpAuthScheme, MediaType, ParameterBuilder, RequestBodyBuilder, ResponseBuilder,
	SecurityScheme,
};
use reinhardt::rest::openapi::{
	EndpointInspector, OpenApiSchema, Operation, ParameterLocation, RefOr, Required, Schema,
	SchemaGenerator,
};
use schemars::transform::{
	ReplaceBoolSchemas, ReplaceConstValue, ReplacePrefixItems, transform_subschemas,
};
use schemars::{
	JsonSchema, Schema as JsonSchemaDefinition, SchemaGenerator as JsonSchemaGenerator,
	generate::SchemaSettings,
};
use serde_json::{Value, from_value, json};
use std::collections::BTreeSet;

pub fn openapi() -> Result<OpenApiSchema> {
	let mut inspector = InspectorConfig::default();
	inspector.security_scheme_names.push("bearer_auth".into());
	let generator = SchemaGenerator::new()
		.title("Aidash API")
		.version(env!("CARGO_PKG_VERSION"))
		.description("Management API for the Aidash agent mesh.");
	let mut document = generator
		.generate()
		.map_err(|e| Error::External(e.to_string()))?;
	document.paths.paths = EndpointInspector::with_config(inspector)
		.extract_paths()
		.map_err(|e| Error::External(e.to_string()))?
		.into_iter()
		.collect();
	let mut contracts = Contracts::default();
	apps::identity::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::execution::capabilities::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::workspaces::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::execution::generation::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::execution::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::marketplace::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::operations::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::federation::peer::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::registry::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::federation::remote::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::knowledge::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::federation::transactions::serializers::openapi::register(&mut contracts, &mut document)?;
	apps::registry::workbench::serializers::openapi::register(&mut contracts, &mut document)?;
	contracts.finish(&mut document)?;
	document
		.components
		.get_or_insert_with(Default::default)
		.add_security_scheme(
			"bearer_auth",
			SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
		);
	Ok(document)
}

/// Enriches existing HTTP-macro operations without maintaining a second router.
pub(crate) struct Contracts {
	schemas: JsonSchemaGenerator,
	response_schemas: JsonSchemaGenerator,
	documented: BTreeSet<(String, String)>,
}
impl Default for Contracts {
	fn default() -> Self {
		let mut boolean_schemas = ReplaceBoolSchemas::default();
		boolean_schemas.skip_additional_properties = true;
		let settings = SchemaSettings::draft2020_12()
			.with(|settings| {
				settings.definitions_path = "/components/schemas".into();
				settings.meta_schema = None;
			})
			.with_transform(boolean_schemas)
			.with_transform(ReplaceConstValue::default())
			.with_transform(ReplacePrefixItems::default())
			.with_transform(native_schema);
		Self {
			response_schemas: settings.clone().for_serialize().into_generator(),
			schemas: settings.with_transform(request_schema).into_generator(),
			documented: BTreeSet::new(),
		}
	}
}

/// Native schema deserialization requires an explicit typeless discriminator.
/// It serializes back to an omitted type, preserving an unrestricted JSON value.
fn native_schema(schema: &mut JsonSchemaDefinition) {
	transform_subschemas(&mut native_schema, schema);
	if let Some(object) = schema.as_object_mut()
		&& !["$ref", "oneOf", "anyOf", "allOf"]
			.iter()
			.any(|key| object.contains_key(*key))
	{
		object.entry("type").or_insert(Value::Null);
	}
}
// Input and output contracts differ for serde defaults and skipped fields.
// Give input definitions an explicit namespace so response fields stay required
// without incorrectly requiring optional/defaulted input fields.
fn request_schema(schema: &mut JsonSchemaDefinition) {
	transform_subschemas(&mut request_schema, schema);
	if let Some(object) = schema.as_object_mut()
		&& let Some(Value::String(reference)) = object.get_mut("$ref")
		&& let Some(name) = reference.strip_prefix("#/components/schemas/")
		&& !name.starts_with("Request_")
	{
		*reference = format!("#/components/schemas/Request_{name}");
	}
}

impl Contracts {
	/// Preserve the public component name of an otherwise inline contract.
	pub fn named<T: JsonSchema>(&mut self, document: &mut OpenApiSchema, name: &str) -> Result<()> {
		let schema = self.schema::<T>(true)?;
		document
			.components
			.get_or_insert_with(Default::default)
			.schemas
			.insert(name.into(), schema);
		Ok(())
	}
	fn schema<T: JsonSchema>(&mut self, response: bool) -> Result<RefOr<Schema>> {
		let generator = if response {
			&mut self.response_schemas
		} else {
			&mut self.schemas
		};
		let mut schema = generator.subschema_for::<T>();
		for transform in generator.transforms_mut() {
			transform.transform(&mut schema);
		}
		from_value(schema.to_value()).map_err(|error| {
			Error::External(format!("OpenAPI schema {}: {error}", T::schema_name()))
		})
	}
	fn operation<'a, E: EndpointInfo>(
		&mut self,
		document: &'a mut OpenApiSchema,
	) -> Result<&'a mut Operation> {
		let first = self
			.documented
			.insert((E::path().into(), E::method().to_string().to_lowercase()));
		let path =
			document.paths.paths.get_mut(E::path()).ok_or_else(|| {
				Error::External(format!("missing OpenAPI endpoint {}", E::path()))
			})?;
		let operation = match E::method().as_str() {
			"GET" => &mut path.get,
			"POST" => &mut path.post,
			"PUT" => &mut path.put,
			"PATCH" => &mut path.patch,
			"DELETE" => &mut path.delete,
			_ => return Err(Error::External("unsupported OpenAPI method".into())),
		};
		let operation = operation
			.as_mut()
			.ok_or_else(|| Error::External(format!("missing OpenAPI method {}", E::name())))?;
		if first {
			operation.responses.responses.clear();
			operation.parameters = None;
			operation.request_body = None;
		}
		Ok(operation)
	}
	pub fn response<E: EndpointInfo, T: JsonSchema>(
		&mut self,
		document: &mut OpenApiSchema,
		_endpoint: impl FnOnce() -> E,
		status: u16,
		media_type: &str,
	) -> Result<()> {
		let schema = self.schema::<T>(true)?;
		let response = ResponseBuilder::new()
			.description(if status < 400 {
				"Successful response"
			} else {
				"Request failed"
			})
			.content(media_type, MediaType::new(Some(schema)))
			.build();
		self.operation::<E>(document)?
			.responses
			.responses
			.insert(status.to_string(), RefOr::T(response));
		Ok(())
	}
	pub fn response_header<E: EndpointInfo, T: JsonSchema>(
		&mut self,
		document: &mut OpenApiSchema,
		_endpoint: impl FnOnce() -> E,
		status: u16,
		name: &str,
		description: &str,
	) -> Result<()> {
		let mut header = Header::new(self.schema::<T>(true)?);
		header.description = Some(description.into());
		let Some(RefOr::T(response)) = self
			.operation::<E>(document)?
			.responses
			.responses
			.get_mut(&status.to_string())
		else {
			return Err(Error::External("missing inline OpenAPI response".into()));
		};
		response.headers.insert(name.into(), header);
		Ok(())
	}
	pub fn empty<E: EndpointInfo>(
		&mut self,
		document: &mut OpenApiSchema,
		_endpoint: impl FnOnce() -> E,
		status: u16,
	) -> Result<()> {
		let response = ResponseBuilder::new()
			.description(if status < 400 {
				"Successful response"
			} else {
				"Request failed"
			})
			.build();
		self.operation::<E>(document)?
			.responses
			.responses
			.insert(status.to_string(), RefOr::T(response));
		Ok(())
	}
	pub fn request<E: EndpointInfo, T: JsonSchema>(
		&mut self,
		document: &mut OpenApiSchema,
		_endpoint: impl FnOnce() -> E,
	) -> Result<()> {
		let schema = self.schema::<T>(false)?;
		self.operation::<E>(document)?.request_body = Some(
			RequestBodyBuilder::new()
				.required(Some(Required::True))
				.content("application/json", MediaType::new(Some(schema)))
				.build(),
		);
		Ok(())
	}
	pub fn binary_request<E: EndpointInfo>(
		&mut self,
		document: &mut OpenApiSchema,
		_endpoint: impl FnOnce() -> E,
	) -> Result<()> {
		let schema = from_value::<RefOr<Schema>>(json!({"type":"string", "format":"binary"}))?;
		self.operation::<E>(document)?.request_body = Some(
			RequestBodyBuilder::new()
				.required(Some(Required::True))
				.content("application/octet-stream", MediaType::new(Some(schema)))
				.build(),
		);
		Ok(())
	}
	pub fn query<E: EndpointInfo, T: JsonSchema>(
		&mut self,
		document: &mut OpenApiSchema,
		_endpoint: impl FnOnce() -> E,
	) -> Result<()> {
		let _ = self.schemas.subschema_for::<T>();
		let mut schema = T::json_schema(&mut self.schemas);
		for transform in self.schemas.transforms_mut() {
			transform.transform(&mut schema);
		}
		let schema = schema.to_value();
		let required = schema["required"].as_array().cloned().unwrap_or_default();
		if let Some(properties) = schema["properties"].as_object() {
			for (name, schema) in properties {
				let parameter = ParameterBuilder::new()
					.name(name)
					.parameter_in(ParameterLocation::Query)
					.required(if required.contains(&Value::String(name.clone())) {
						Required::True
					} else {
						Required::False
					})
					.schema(Some(from_value::<RefOr<Schema>>(schema.clone())?))
					.build();
				self.operation::<E>(document)?
					.parameters
					.get_or_insert_with(Vec::new)
					.push(parameter);
			}
		}
		Ok(())
	}
	pub fn path<E: EndpointInfo>(
		&mut self,
		document: &mut OpenApiSchema,
		_endpoint: impl FnOnce() -> E,
		formats: &[&str],
	) -> Result<()> {
		let names = E::path()
			.split('/')
			.filter_map(|part| part.strip_prefix('{').and_then(|p| p.strip_suffix('}')));
		for (name, format) in names.zip(formats) {
			let schema = if *format == "Uuid" {
				json!({"type":"string","format":"uuid"})
			} else {
				json!({"type":"string"})
			};
			let parameter = ParameterBuilder::new()
				.name(name)
				.parameter_in(ParameterLocation::Path)
				.required(Required::True)
				.schema(Some(from_value::<RefOr<Schema>>(schema)?))
				.build();
			self.operation::<E>(document)?
				.parameters
				.get_or_insert_with(Vec::new)
				.push(parameter);
		}
		Ok(())
	}
	pub fn idempotency<E: EndpointInfo>(
		&mut self,
		document: &mut OpenApiSchema,
		_endpoint: impl FnOnce() -> E,
	) -> Result<()> {
		let parameter = ParameterBuilder::new()
			.name("Idempotency-Key")
			.parameter_in(ParameterLocation::Header)
			.required(Required::False)
			.schema(Some(from_value::<RefOr<Schema>>(
				json!({"type":"string","format":"uuid"}),
			)?))
			.build();
		self.operation::<E>(document)?
			.parameters
			.get_or_insert_with(Vec::new)
			.push(parameter);
		Ok(())
	}
	fn finish(&mut self, document: &mut OpenApiSchema) -> Result<()> {
		let components = document.components.get_or_insert_with(Default::default);
		for (name, schema) in self.response_schemas.take_definitions(true) {
			let schema = from_value(schema)
				.map_err(|error| Error::External(format!("OpenAPI definition {name}: {error}")))?;
			components.schemas.insert(name, schema);
		}
		for (name, schema) in self.schemas.take_definitions(true) {
			let schema: RefOr<Schema> = from_value(schema)
				.map_err(|error| Error::External(format!("OpenAPI definition {name}: {error}")))?;
			// The dashboard imports legacy component names for input-only types.
			// Their defaults must keep deserialize semantics. Where an output of
			// the same name exists, its serialize contract remains authoritative.
			components
				.schemas
				.entry(name.clone())
				.or_insert_with(|| schema.clone());
			components.schemas.insert(format!("Request_{name}"), schema);
		}
		document.paths.paths.retain(|path, item| {
			for (method, operation) in [
				("get", &mut item.get),
				("post", &mut item.post),
				("put", &mut item.put),
				("patch", &mut item.patch),
				("delete", &mut item.delete),
			] {
				if !self.documented.contains(&(path.clone(), method.into())) {
					*operation = None;
				}
			}
			[&item.get, &item.post, &item.put, &item.patch, &item.delete]
				.iter()
				.any(|x| x.is_some())
		});
		Ok(())
	}
}
