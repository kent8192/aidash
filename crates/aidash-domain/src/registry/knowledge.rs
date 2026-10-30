//! Private reference text invariants and immutable content identity.
use super::ReferenceDocument;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Total original image bytes accepted in one private reference set.
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Identify image-bearing private documents before a copy or transfer.
pub fn contains_images(documents: &Value) -> bool {
	documents.as_array().is_some_and(|documents| {
		documents.iter().any(|document| {
			document
				.get("data_base64")
				.is_some_and(|value| !value.is_null())
				|| document["media_type"]
					.as_str()
					.is_some_and(|media_type| media_type.starts_with("image/"))
		})
	})
}

/// Validated bytes remain ephemeral until the owning private row is written.
pub struct ValidatedImage {
	pub document_index: usize,
	pub media_type: String,
	pub bytes: Vec<u8>,
}

/// Validate the entire reference set before the first embedding request.
pub fn validated_images(documents: &[ReferenceDocument]) -> Result<Vec<ValidatedImage>> {
	if documents.is_empty() || documents.len() > 8 {
		return Err(Error::Invalid("invalid reference document count".into()));
	}
	let mut images = Vec::new();
	let mut image_total = 0_usize;
	let mut text_total = 0_usize;
	for (document_index, document) in documents.iter().enumerate() {
		if document.image_digest.is_some() || document.image_size.is_some() {
			return Err(Error::Invalid(
				"reference image metadata is read-only".into(),
			));
		}
		if document.name.contains('\0') {
			return Err(Error::Invalid("reference document names and text must not contain NUL characters; decode text as UTF-8 before upload".into()));
		}
		if document.name.trim().is_empty() || document.name.len() > 255 {
			return Err(Error::Invalid("invalid reference document name".into()));
		}
		if let Some(bytes) = image_bytes(document)? {
			image_total = image_total
				.checked_add(bytes.len())
				.ok_or_else(|| Error::Invalid("reference image limit exceeded".into()))?;
			if image_total > MAX_IMAGE_BYTES {
				return Err(Error::Invalid("reference image limit exceeded".into()));
			}
			images.push(ValidatedImage {
				document_index,
				media_type: document.media_type.clone(),
				bytes,
			});
		} else {
			validate_text(std::slice::from_ref(document))?;
			text_total = text_total
				.checked_add(document.text.len())
				.ok_or_else(|| Error::Invalid("reference text limit exceeded".into()))?;
		}
	}
	if text_total > 65536 {
		return Err(Error::Invalid("reference text limit exceeded".into()));
	}
	Ok(images)
}

/// Separate image bytes from the source document identity and run context.
pub fn canonical_documents(
	documents: &[ReferenceDocument],
) -> Result<(Vec<ReferenceDocument>, Value)> {
	let images = validated_images(documents)?;
	let mut canonical = documents.to_vec();
	let mut stored = Vec::with_capacity(images.len());
	for image in images {
		let document = &mut canonical[image.document_index];
		let bytes = document
			.data_base64
			.take()
			.expect("validated image has bytes");
		document.image_size = Some(image.bytes.len());
		document.image_digest = Some(crate::semantic::indexing::content_digest_bytes(
			&image.bytes,
		));
		stored.push(
			serde_json::json!({"document_index": image.document_index, "data_base64": bytes}),
		);
	}
	Ok((canonical, Value::Array(stored)))
}

/// A bounded vector collection belonging to one immutable Agent version.
/// Source bytes stay in the private documents, never in these points.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageCollection {
	pub agent_id: String,
	pub agent_version: String,
	pub embedding: super::Entry,
	pub points: Vec<ImagePoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImagePoint {
	pub document_index: usize,
	pub name: String,
	pub media_type: String,
	pub size: usize,
	pub digest: String,
	pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageMatch {
	pub document_index: usize,
	pub name: String,
	pub media_type: String,
	pub size: usize,
	pub digest: String,
	pub score: f64,
}

fn magnitude(vector: &[f32], dimensions: usize) -> Result<f64> {
	if vector.len() != dimensions || vector.iter().any(|value| !value.is_finite()) {
		return Err(Error::Invalid("invalid reference embedding vector".into()));
	}
	let magnitude = vector
		.iter()
		.map(|value| f64::from(*value).powi(2))
		.sum::<f64>()
		.sqrt();
	if !magnitude.is_finite() || magnitude == 0.0 {
		return Err(Error::Invalid("invalid reference embedding vector".into()));
	}
	Ok(magnitude)
}

impl ImageCollection {
	pub fn new(
		agent_id: &str,
		agent_version: &str,
		embedding: super::Entry,
		documents: &[ReferenceDocument],
		vectors: Vec<Vec<f32>>,
	) -> Result<Self> {
		if agent_id.is_empty()
			|| agent_id.len() > 100
			|| semver::Version::parse(agent_version).is_err()
			|| embedding.kind != "embedding"
			|| documents.is_empty()
			|| documents.len() > 8
		{
			return Err(Error::Invalid("invalid reference image collection".into()));
		}
		let config: crate::semantic::EmbeddingConfig =
			serde_json::from_value(embedding.config.clone())?;
		config.validate_parameters()?;
		if !config
			.inputs
			.contains(&crate::semantic::EmbeddingInputKind::Image)
		{
			return Err(Error::Invalid(
				"reference embedding does not accept images".into(),
			));
		}
		let images = validated_images(documents)?;
		if images.is_empty() || vectors.len() != images.len() {
			return Err(Error::Invalid(
				"invalid reference image embedding count".into(),
			));
		}
		let mut points = Vec::with_capacity(images.len());
		for (image, vector) in images.into_iter().zip(vectors) {
			magnitude(&vector, config.dimensions)?;
			points.push(ImagePoint {
				document_index: image.document_index,
				name: documents[image.document_index].name.clone(),
				media_type: image.media_type,
				size: image.bytes.len(),
				digest: crate::semantic::indexing::content_digest_bytes(&image.bytes),
				vector,
			});
		}
		Ok(Self {
			agent_id: agent_id.into(),
			agent_version: agent_version.into(),
			embedding,
			points,
		})
	}

	pub fn search(
		&self,
		agent_id: &str,
		agent_version: &str,
		query: &[f32],
		limit: usize,
	) -> Result<Vec<ImageMatch>> {
		if self.agent_id != agent_id
			|| self.agent_version != agent_version
			|| limit == 0
			|| limit > 8
		{
			return Err(Error::Invalid(
				"invalid reference collection scope or limit".into(),
			));
		}
		let config: crate::semantic::EmbeddingConfig =
			serde_json::from_value(self.embedding.config.clone())?;
		let query_norm = magnitude(query, config.dimensions)?;
		let mut matches = Vec::with_capacity(self.points.len());
		for point in &self.points {
			let norm = magnitude(&point.vector, config.dimensions)?;
			let dot = query
				.iter()
				.zip(&point.vector)
				.map(|(a, b)| f64::from(*a) * f64::from(*b))
				.sum::<f64>();
			matches.push(ImageMatch {
				document_index: point.document_index,
				name: point.name.clone(),
				media_type: point.media_type.clone(),
				size: point.size,
				digest: point.digest.clone(),
				score: dot / (query_norm * norm),
			});
		}
		matches.sort_by(|a, b| {
			b.score
				.total_cmp(&a.score)
				.then_with(|| a.document_index.cmp(&b.document_index))
		});
		matches.truncate(limit);
		Ok(matches)
	}

	/// Recheck the immutable Agent scope, pinned definition and image metadata
	/// before any stored vector can become a disclosed reference match.
	pub fn verify(
		&self,
		agent_id: &str,
		agent_version: &str,
		embedding: &super::Entry,
		documents: &[ReferenceDocument],
	) -> Result<()> {
		let config: crate::semantic::EmbeddingConfig =
			serde_json::from_value(embedding.config.clone())?;
		config.validate_parameters()?;
		if embedding.kind != "embedding"
			|| !config
				.inputs
				.contains(&crate::semantic::EmbeddingInputKind::Image)
		{
			return Err(Error::Invalid(
				"reference embedding does not accept images".into(),
			));
		}
		let valid_points = self.points.iter().all(|point| {
			let Some(document) = documents.get(point.document_index) else {
				return false;
			};
			document.data_base64.is_none()
				&& document.media_type.starts_with("image/")
				&& document.name == point.name
				&& document.media_type == point.media_type
				&& document.image_size == Some(point.size)
				&& document.image_digest.as_deref() == Some(point.digest.as_str())
				&& magnitude(&point.vector, config.dimensions).is_ok()
		});
		let image_count = documents
			.iter()
			.filter(|d| d.image_digest.is_some())
			.count();
		if self.points.is_empty()
			|| self.agent_id != agent_id
			|| self.agent_version != agent_version
			|| serde_json::to_value(&self.embedding)? != serde_json::to_value(embedding)?
			|| self.points.len() != image_count
			|| !valid_points
			|| self
				.points
				.windows(2)
				.any(|points| points[0].document_index >= points[1].document_index)
		{
			return Err(Error::Invalid(
				"reference image collection changed; register a new agent version".into(),
			));
		}
		Ok(())
	}
}

/// Validate the original bytes without putting them in a vector or context.
pub fn image_bytes(document: &ReferenceDocument) -> Result<Option<Vec<u8>>> {
	use base64::{Engine as _, engine::general_purpose::STANDARD};
	let Some(encoded) = &document.data_base64 else {
		return Ok(None);
	};
	if encoded.len() > MAX_IMAGE_BYTES.div_ceil(3) * 4 || !document.text.is_empty() {
		return Err(Error::Invalid(
			"invalid or oversized reference image".into(),
		));
	}
	let bytes = STANDARD
		.decode(encoded)
		.map_err(|_| Error::Invalid("invalid reference image encoding".into()))?;
	if bytes.len() > MAX_IMAGE_BYTES {
		return Err(Error::Invalid(
			"invalid or oversized reference image".into(),
		));
	}
	crate::provider::validate_image(&document.media_type, &bytes)?;
	Ok(Some(bytes))
}

/// Text remains available as before; an image contributes only its name
/// until the Agent explicitly opens a retrieved match.
pub fn visible_documents(documents: &[ReferenceDocument]) -> Result<Value> {
	let mut visible = Vec::with_capacity(documents.len());
	for document in documents {
		if document.data_base64.is_some() || document.media_type.starts_with("image/") {
			visible.push(serde_json::json!({"name":document.name}));
		} else {
			visible.push(serde_json::to_value(document)?);
		}
	}
	Ok(Value::Array(visible))
}

pub fn validate(documents: &[ReferenceDocument]) -> Result<()> {
	validated_images(documents).map(|_| ())
}

fn validate_text(documents: &[ReferenceDocument]) -> Result<()> {
	if documents.is_empty()
		|| documents.len() > 8
		|| documents.iter().map(|d| d.text.len()).sum::<usize>() > 65536
	{
		return Err(Error::Invalid(
			"attach 1..8 reference documents with at most 64 KiB of extracted text in total".into(),
		));
	}
	for d in documents {
		if d.data_base64.is_some() {
			return Err(Error::Invalid("unexpected image in text validation".into()));
		}
		if d.name.contains('\0') || d.text.contains('\0') {
			return Err(Error::Invalid("reference document names and text must not contain NUL characters; decode text as UTF-8 before upload".into()));
		}
		if d.name.trim().is_empty()
			|| d.name.len() > 255
			|| d.text.trim().is_empty()
			|| !matches!(
				d.media_type.as_str(),
				"application/pdf"
					| "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
					| "text/plain"
			) {
			return Err(Error::Invalid(
				"invalid or empty reference document; scanned PDFs require OCR before upload"
					.into(),
			));
		}
	}
	Ok(())
}

pub fn digest(value: &Value) -> String {
	format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}
#[cfg(test)]
mod tests;
