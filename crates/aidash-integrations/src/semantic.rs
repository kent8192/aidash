//! Explicit provider adapters. Neither backend is an authorization authority.
use crate::{Error, Result};
use aidash_application::ports::{Credentials, EmbeddingProvider, VectorIndex};
use aidash_domain::semantic::{
	Embedding, EmbeddingConfig, Point, VectorConfig, VectorFilter as Filter,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Own the connection pool with the node runtime. A process-global pool can
/// retain dispatch tasks from a runtime that has already shut down.
pub fn client() -> Result<reqwest::Client> {
	reqwest::Client::builder()
		.timeout(Duration::from_secs(20))
		.connect_timeout(Duration::from_secs(3))
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.map_err(crate::http_error)
}
fn request(
	client: &reqwest::Client,
	method: reqwest::Method,
	endpoint: &str,
	path: &str,
) -> Result<reqwest::RequestBuilder> {
	crate::compaction::validate_endpoint(endpoint)?;
	Ok(client.request(method, format!("{}{path}", endpoint.trim_end_matches('/'))))
}
fn credential(
	credentials: &dyn Credentials,
	request: reqwest::RequestBuilder,
	name: &Option<String>,
	vector: bool,
) -> Result<reqwest::RequestBuilder> {
	match name {
		Some(name) => {
			let key = credentials.resolve(name)?;
			Ok(if vector {
				request.header("api-key", key)
			} else {
				request.bearer_auth(key)
			})
		}
		None => Ok(request),
	}
}
async fn response(request: reqwest::RequestBuilder) -> Result<Value> {
	let response = request.send().await.map_err(crate::http_error)?;
	if !response.status().is_success() {
		return Err(Error::External(format!(
			"semantic backend returned HTTP {}",
			response.status().as_u16()
		)));
	}
	crate::response::json(response, 1_048_576).await
}

pub async fn embed(
	credentials: &dyn Credentials,
	client: &reqwest::Client,
	config: &EmbeddingConfig,
	text: &str,
) -> Result<Embedding> {
	#[derive(Deserialize)]
	struct EmbeddingDatum {
		index: usize,
		embedding: Vec<f32>,
	}
	#[derive(Deserialize)]
	struct Embeddings {
		model: String,
		data: Vec<EmbeddingDatum>,
	}
	if config.provider != "openai" {
		return Err(Error::Invalid("unsupported embedding provider".into()));
	}
	let request = credential(
		credentials,
		request(
			client,
			reqwest::Method::POST,
			&config.endpoint,
			"/embeddings",
		)?,
		&config.credential_env,
		false,
	)?
	.json(&json!({"model":config.model,"input":text,"encoding_format":"float"}));
	let response = request.send().await.map_err(crate::http_error)?;
	if !response.status().is_success() {
		use aidash_domain::semantic::Failure;
		return Err(Error::RemoteSemantic(match response.status().as_u16() {
			408 | 429 | 500..=599 => Failure::Unavailable,
			401 | 403 => Failure::Configuration,
			_ => Failure::ProviderContract,
		}));
	}
	let value: Value = crate::response::json(response, 1_048_576)
		.await
		.map_err(|_| Error::RemoteSemantic(aidash_domain::semantic::Failure::ProviderContract))?;
	let prompt = value["usage"]["prompt_tokens"].as_u64();
	let total = value["usage"]["total_tokens"].as_u64();
	let tokens = prompt.filter(|count| *count > 0 && Some(*count) == total);
	let output: Embeddings = serde_json::from_value(value)
		.map_err(|_| Error::RemoteSemantic(aidash_domain::semantic::Failure::ProviderContract))?;
	if output.model != config.model || output.data.len() != 1 || output.data[0].index != 0 {
		return Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::ProviderContract,
		));
	}
	let vector = output
		.data
		.into_iter()
		.next()
		.expect("validated length")
		.embedding;
	let magnitude: f64 = vector.iter().map(|x| f64::from(*x).powi(2)).sum();
	if vector.len() != config.dimensions
		|| vector.iter().any(|x| !x.is_finite())
		|| !magnitude.is_finite()
		|| magnitude <= 0.0
	{
		return Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::ProviderContract,
		));
	}
	Ok(Embedding { vector, tokens })
}
fn collection_path(collection: &str) -> Result<String> {
	if !collection.starts_with("aidash_")
		|| !collection
			.bytes()
			.all(|c| c.is_ascii_alphanumeric() || c == b'_')
	{
		return Err(Error::Invalid(
			"invalid semantic collection identity".into(),
		));
	}
	Ok(format!("/collections/{collection}"))
}
fn vector_request(
	credentials: &dyn Credentials,
	client: &reqwest::Client,
	config: &VectorConfig,
	method: reqwest::Method,
	path: &str,
) -> Result<reqwest::RequestBuilder> {
	if config.provider != "qdrant" {
		return Err(Error::Invalid("unsupported vector provider".into()));
	}
	credential(
		credentials,
		request(client, method, &config.endpoint, path)?,
		&config.credential_env,
		true,
	)
}
pub async fn ensure_collection(
	credentials: &dyn Credentials,
	client: &reqwest::Client,
	config: &VectorConfig,
	collection: &str,
	dimensions: usize,
) -> Result<()> {
	let path = collection_path(collection)?;
	let get = vector_request(credentials, client, config, reqwest::Method::GET, &path)?
		.send()
		.await
		.map_err(crate::http_error)?;
	if get.status() == reqwest::StatusCode::NOT_FOUND {
		// Concurrent retry after an unknown create result is safe. Verify the
		// actual width/distance below, including when another creator won.
		let result = vector_request(credentials, client, config, reqwest::Method::PUT, &path)?
			.json(&json!({"vectors":{"size":dimensions,"distance":"Cosine"}}))
			.send()
			.await
			.map_err(crate::http_error)?;
		if !result.status().is_success() && result.status() != reqwest::StatusCode::CONFLICT {
			return Err(Error::External("cannot create semantic collection".into()));
		}
	} else if !get.status().is_success() {
		return Err(Error::External("cannot inspect semantic collection".into()));
	}
	let actual = response(vector_request(
		credentials,
		client,
		config,
		reqwest::Method::GET,
		&path,
	)?)
	.await?;
	let vector = &actual["result"]["config"]["params"]["vectors"];
	if vector["size"].as_u64() != Some(dimensions as u64) || vector["distance"] != "Cosine" {
		return Err(Error::External(
			"semantic collection schema mismatch".into(),
		));
	}
	Ok(())
}
pub async fn upsert(
	credentials: &dyn Credentials,
	client: &reqwest::Client,
	config: &VectorConfig,
	collection: &str,
	point: Uuid,
	vector: &[f32],
	payload: Value,
) -> Result<()> {
	let path = format!(
		"{}/points?wait=true&ordering=strong",
		collection_path(collection)?
	);
	let result = response(
		vector_request(credentials, client, config, reqwest::Method::PUT, &path)?
			.json(&json!({"points":[{"id":point,"vector":vector,"payload":payload}]})),
	)
	.await?;
	if result["result"]["status"] != "completed" {
		return Err(Error::External("vector upsert did not complete".into()));
	}
	Ok(())
}
pub async fn delete_point(
	credentials: &dyn Credentials,
	client: &reqwest::Client,
	config: &VectorConfig,
	collection: &str,
	point: Uuid,
) -> Result<()> {
	let path = format!(
		"{}/points/delete?wait=true&ordering=strong",
		collection_path(collection)?
	);
	let response = vector_request(credentials, client, config, reqwest::Method::POST, &path)?
		.json(&json!({"points":[point]}))
		.send()
		.await
		.map_err(crate::http_error)?;
	if response.status() == reqwest::StatusCode::NOT_FOUND {
		return Ok(());
	}
	if !response.status().is_success() {
		return Err(Error::External("vector deletion failed".into()));
	}
	let result: Value = crate::response::json(response, 1_048_576).await?;
	if result["result"]["status"] != "completed" {
		return Err(Error::External("vector deletion did not complete".into()));
	}
	Ok(())
}
pub async fn delete_collection(
	credentials: &dyn Credentials,
	client: &reqwest::Client,
	config: &VectorConfig,
	collection: &str,
) -> Result<()> {
	let response = vector_request(
		credentials,
		client,
		config,
		reqwest::Method::DELETE,
		&collection_path(collection)?,
	)?
	.send()
	.await
	.map_err(crate::http_error)?;
	if response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND {
		Ok(())
	} else {
		Err(Error::External(
			"semantic collection deletion failed".into(),
		))
	}
}

pub async fn query(
	credentials: &dyn Credentials,
	client: &reqwest::Client,
	config: &VectorConfig,
	collection: &str,
	vector: &[f32],
	filter: Filter<'_>,
	limit: usize,
) -> Result<Vec<Point>> {
	let Filter {
		allowed,
		workspace,
		tenant,
	} = filter;
	if allowed.is_empty() {
		return Ok(vec![]);
	}
	let path = format!(
		"{}/points/query?consistency=all",
		collection_path(collection)?
	);
	let value=response(vector_request(credentials, client, config,reqwest::Method::POST,&path)?
        .json(&json!({"query":vector,"filter":{"must":[{"has_id":allowed},{"key":"workspace_id","match":{"value":workspace.to_string()}},{"key":"tenant","match":{"value":tenant}}]},"limit":limit,"with_payload":true,"with_vector":false}))).await?;
	let result: Vec<Point> = serde_json::from_value(value["result"]["points"].clone())
		.map_err(|_| Error::RemoteSemantic(aidash_domain::semantic::Failure::ProviderContract))?;
	if result.len() > limit || result.iter().any(|p| !p.score.is_finite()) {
		return Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::ProviderContract,
		));
	}
	Ok(result)
}

pub async fn present(
	credentials: &dyn Credentials,
	client: &reqwest::Client,
	config: &VectorConfig,
	collection: &str,
	ids: &[Uuid],
) -> Result<bool> {
	if ids.is_empty() {
		return Ok(true);
	}
	let path = format!("{}/points?consistency=all", collection_path(collection)?);
	let response = vector_request(credentials, client, config, reqwest::Method::POST, &path)?
		.json(&json!({"ids":ids,"with_payload":false,"with_vector":false}))
		.send()
		.await
		.map_err(crate::http_error)?;
	if response.status() == reqwest::StatusCode::NOT_FOUND {
		return Ok(false);
	}
	if !response.status().is_success() {
		return Err(Error::External("cannot verify indexed points".into()));
	}
	let value: Value = crate::response::json(response, 1_048_576).await?;
	let Some(rows) = value["result"].as_array() else {
		return Err(Error::RemoteSemantic(
			aidash_domain::semantic::Failure::ProviderContract,
		));
	};
	let mut actual = std::collections::BTreeSet::new();
	for row in rows {
		let id = row["id"]
			.as_str()
			.and_then(|id| id.parse::<Uuid>().ok())
			.ok_or(Error::RemoteSemantic(
				aidash_domain::semantic::Failure::ProviderContract,
			))?;
		if !ids.contains(&id) || !actual.insert(id) {
			return Err(Error::RemoteSemantic(
				aidash_domain::semantic::Failure::ProviderContract,
			));
		}
	}
	Ok(actual.len() == ids.len() && ids.iter().all(|id| actual.contains(id)))
}

use serde::Deserialize;

/// Connections and credentials are provided by the process composition root.
#[derive(Clone)]
pub struct SemanticClient {
	pub client: reqwest::Client,
	pub credentials: Arc<dyn Credentials>,
}
#[async_trait]
impl EmbeddingProvider for SemanticClient {
	async fn embed(&self, config: &EmbeddingConfig, text: &str) -> Result<Embedding> {
		embed(self.credentials.as_ref(), &self.client, config, text).await
	}
}
#[async_trait]
impl VectorIndex for SemanticClient {
	async fn ensure_collection(
		&self,
		config: &VectorConfig,
		collection: &str,
		dimensions: usize,
	) -> Result<()> {
		ensure_collection(
			self.credentials.as_ref(),
			&self.client,
			config,
			collection,
			dimensions,
		)
		.await
	}
	async fn upsert(
		&self,
		config: &VectorConfig,
		collection: &str,
		point: Uuid,
		vector: &[f32],
		payload: Value,
	) -> Result<()> {
		upsert(
			self.credentials.as_ref(),
			&self.client,
			config,
			collection,
			point,
			vector,
			payload,
		)
		.await
	}
	async fn delete_point(
		&self,
		config: &VectorConfig,
		collection: &str,
		point: Uuid,
	) -> Result<()> {
		delete_point(
			self.credentials.as_ref(),
			&self.client,
			config,
			collection,
			point,
		)
		.await
	}
	async fn delete_collection(&self, config: &VectorConfig, collection: &str) -> Result<()> {
		delete_collection(self.credentials.as_ref(), &self.client, config, collection).await
	}
	async fn query(
		&self,
		config: &VectorConfig,
		collection: &str,
		vector: &[f32],
		filter: Filter<'_>,
		limit: usize,
	) -> Result<Vec<Point>> {
		query(
			self.credentials.as_ref(),
			&self.client,
			config,
			collection,
			vector,
			filter,
			limit,
		)
		.await
	}
	async fn present(&self, config: &VectorConfig, collection: &str, ids: &[Uuid]) -> Result<bool> {
		present(
			self.credentials.as_ref(),
			&self.client,
			config,
			collection,
			ids,
		)
		.await
	}
}
