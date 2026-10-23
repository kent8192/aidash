//! Admit a rendered page against the same model-media budget before it is recorded.
use crate::{execution::check_model_media_headroom, ports::research::PageRenderer};
use aidash_domain::{
	model::ModelConfig,
	provider::ContentPart,
	research::{Failure, citations::EvidenceRef, document::RenderedPage},
};
use async_trait::async_trait;

pub struct FittingRenderer<'a> {
	pub renderer: &'a dyn PageRenderer,
	pub model: &'a ModelConfig,
	pub headroom: usize,
	pub selected_parts: Vec<ContentPart>,
}

#[async_trait]
impl PageRenderer for FittingRenderer<'_> {
	async fn render(&self, pdf: Vec<u8>, page: u32) -> std::result::Result<RenderedPage, Failure> {
		if self
			.selected_parts
			.iter()
			.filter(|part| matches!(part, ContentPart::Image { .. } | ContentPart::Audio { .. }))
			.count() >= 8
		{
			return Err(Failure::ResponseTooLarge);
		}
		let image = self.renderer.render(pdf, page).await?;
		image.validate().map_err(|_| Failure::RenderFailed)?;
		let held: usize = self
			.selected_parts
			.iter()
			.map(|part| match part {
				ContentPart::Image { bytes, .. } | ContentPart::Audio { bytes, .. } => bytes.len(),
				ContentPart::Text(_) => 0,
			})
			.sum();
		if held.saturating_add(image.bytes.len()) > 8 * 1024 * 1024 {
			return Err(Failure::ResponseTooLarge);
		}
		let mut parts = self.selected_parts.clone();
		// Evidence references have a fixed-length UUID representation; use its
		// exact framing before the store allocates the actual fragment ID.
		parts.push(ContentPart::Text(format!(
			"Page image: {}",
			EvidenceRef(uuid::Uuid::nil())
		)));
		parts.push(
			ContentPart::from_media(&image.media_type, image.bytes.clone())
				.map_err(|_| Failure::RenderFailed)?,
		);
		check_model_media_headroom(self.headroom, parts, self.model)
			.map_err(|_| Failure::ResponseTooLarge)?;
		Ok(image)
	}
}
