//! PDF reading for Page reading (#312, ADR 0053). `hayro` parses PDFs in pure
//! Rust but bounds neither decompression nor time, so Aidash parses a PDF only
//! in a child process under operating-system limits ([`ChildEngine`]): the
//! `aidash` binary started with [`CHILD_COMMAND`] reads one PDF on standard
//! input and answers on standard output. Only page content streams and the
//! fonts they use are read; annotations, forms, scripts and attachments are
//! never interpreted.
use aidash_application::ports::research::PageRenderer;
use aidash_domain::research::{
	Failure,
	document::{
		Completeness, Line, MAX_LINE_BYTES, MAX_LINES, MAX_PAGE_IMAGE_BYTES, MAX_PAGE_IMAGE_EDGE,
		MAX_SNAPSHOT_BYTES, RenderedPage,
	},
};
use async_trait::async_trait;
use hayro::{
	PixmapSettings, RenderCache, RenderSettings,
	hayro_interpret::{
		BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
		InterpreterCache, InterpreterSettings, SoftMask, TransformExt, font::GlyphRun,
		interpret_page,
	},
	hayro_syntax::{
		LoadPdfError, Pdf,
		object::{Dict, Name, Object},
	},
	kurbo::{Affine, BezPath, Point, Rect},
};
use serde::{Deserialize, Serialize};
use std::{
	io::{Read, Write},
	path::PathBuf,
	time::Duration,
};

/// The largest PDF Page reading accepts.
pub const MAX_PDF_BYTES: usize = 16 * 1024 * 1024;
/// Pages read from one PDF; later pages make the snapshot incomplete.
pub const MAX_PAGES: usize = 500;
/// A page with fewer non-whitespace characters of text carries no text.
pub const MIN_PAGE_CHARACTERS: usize = 16;
/// The hidden command of the `aidash` binary that runs one PDF job.
pub const CHILD_COMMAND: &str = "__research-pdf";

/// The text of one PDF, as snapshot lines with their 1-based pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfText {
	pub title: Option<String>,
	pub page_count: u32,
	pub lines: Vec<Line>,
	/// 1-based pages that carry no text, in page order.
	pub pages_without_text: Vec<u32>,
	pub completeness: Completeness,
}

/// What the child writes on standard output.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reply {
	Text(PdfText),
	Page(RenderedPage),
	Failure(Failure),
}

/// Extracts the text of a PDF and renders its pages.
#[async_trait]
pub trait PdfEngine: Send + Sync {
	async fn extract(&self, pdf: Vec<u8>) -> Result<PdfText, Failure>;
	async fn render(&self, pdf: Vec<u8>, page: u32) -> Result<RenderedPage, Failure>;
}

/// The production engine: one bounded child process per PDF.
pub struct ChildEngine {
	program: PathBuf,
	limits: aidash_runtime::child::Limits,
}

impl ChildEngine {
	/// Children run `program`, which must be the `aidash` binary.
	pub fn new(program: PathBuf) -> Self {
		Self {
			program,
			limits: aidash_runtime::child::Limits {
				cpu_seconds: 20,
				memory_bytes: 1024 * 1024 * 1024,
				deadline: Duration::from_secs(20),
				// Room for the text reply or a base64 page image.
				max_output: (2 * MAX_SNAPSHOT_BYTES).max(2 * MAX_PAGE_IMAGE_BYTES) + 64 * 1024,
			},
		}
	}
	/// Run one child job and decode its reply. A child that fails for any
	/// reason (a limit, a crash or a malformed reply) yields `failed`.
	async fn job(&self, args: &[&str], pdf: &[u8], failed: Failure) -> Result<Reply, Failure> {
		let output = aidash_runtime::child::run(&self.program, args, pdf, self.limits)
			.await
			.map_err(|_| failed)?;
		match serde_json::from_slice(&output) {
			Ok(Reply::Failure(failure)) => Err(failure),
			Ok(reply) => Ok(reply),
			Err(_) => Err(failed),
		}
	}
	/// Children run this process's own binary.
	pub fn current_exe() -> std::io::Result<Self> {
		std::env::current_exe().map(Self::new)
	}
}

#[async_trait]
impl PdfEngine for ChildEngine {
	async fn extract(&self, pdf: Vec<u8>) -> Result<PdfText, Failure> {
		match self
			.job(&[CHILD_COMMAND, "extract"], &pdf, Failure::ExtractionFailed)
			.await?
		{
			Reply::Text(text) => Ok(text),
			_ => Err(Failure::ExtractionFailed),
		}
	}
	async fn render(&self, pdf: Vec<u8>, page: u32) -> Result<RenderedPage, Failure> {
		let page = page.to_string();
		match self
			.job(
				&[CHILD_COMMAND, "render", &page],
				&pdf,
				Failure::RenderFailed,
			)
			.await?
		{
			Reply::Page(image) => Ok(image),
			_ => Err(Failure::RenderFailed),
		}
	}
}

#[async_trait]
impl PageRenderer for ChildEngine {
	async fn render(&self, pdf: Vec<u8>, page: u32) -> Result<RenderedPage, Failure> {
		PdfEngine::render(self, pdf, page).await
	}
}

/// Parses in this process. Only for tests, whose fixtures are trusted.
#[cfg(any(test, feature = "test-support"))]
pub struct InProcessEngine;

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl PdfEngine for InProcessEngine {
	async fn extract(&self, pdf: Vec<u8>) -> Result<PdfText, Failure> {
		extract(&pdf)
	}
	async fn render(&self, pdf: Vec<u8>, page: u32) -> Result<RenderedPage, Failure> {
		render(&pdf, page)
	}
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl PageRenderer for InProcessEngine {
	async fn render(&self, pdf: Vec<u8>, page: u32) -> Result<RenderedPage, Failure> {
		render(&pdf, page)
	}
}

/// Run one PDF job when this process was started as the PDF child. Returns
/// the exit status, or `None` for any other invocation.
pub fn child_main(args: &[String]) -> Option<i32> {
	if args.get(1).map(String::as_str) != Some(CHILD_COMMAND) {
		return None;
	}
	let mut pdf = Vec::new();
	if std::io::stdin()
		.take(MAX_PDF_BYTES as u64 + 1)
		.read_to_end(&mut pdf)
		.is_err()
		|| pdf.len() > MAX_PDF_BYTES
	{
		return Some(2);
	}
	let reply = match args.get(2).map(String::as_str) {
		Some("extract") => match extract(&pdf) {
			Ok(text) => Reply::Text(text),
			Err(failure) => Reply::Failure(failure),
		},
		Some("render") => {
			let Some(page) = args.get(3).and_then(|page| page.parse().ok()) else {
				return Some(2);
			};
			match render(&pdf, page) {
				Ok(image) => Reply::Page(image),
				Err(failure) => Reply::Failure(failure),
			}
		}
		_ => return Some(2),
	};
	let Ok(encoded) = serde_json::to_vec(&reply) else {
		return Some(1);
	};
	let mut stdout = std::io::stdout().lock();
	Some(i32::from(
		stdout.write_all(&encoded).is_err() || stdout.flush().is_err(),
	))
}

/// The text of `pdf`, page by page. Call it only inside the bounded child.
pub fn extract(pdf: &[u8]) -> Result<PdfText, Failure> {
	let pdf = open(pdf)?;
	let settings = InterpreterSettings {
		render_annotations: false,
		..InterpreterSettings::default()
	};
	let cache = InterpreterCache::new();
	let pages = pdf.pages();
	let mut lines = Vec::new();
	let mut bytes = 0usize;
	let mut pages_without_text = Vec::new();
	let mut completeness = Completeness::Complete;
	'pages: for (index, page) in pages.iter().enumerate() {
		if index == MAX_PAGES {
			completeness = Completeness::TextLimit;
			break;
		}
		let number = u32::try_from(index + 1).map_err(|_| Failure::ExtractionFailed)?;
		let (width, height) = page.render_dimensions();
		let mut context = Context::new(
			page.initial_transform(true).to_kurbo(),
			Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
			&cache,
			page.xref(),
			settings.clone(),
		);
		let mut device = TextDevice::default();
		interpret_page(page, &mut context, &mut device);
		let page_lines = device.finish();
		let characters = page_lines
			.iter()
			.map(|line| line.chars().filter(|c| !c.is_whitespace()).count())
			.sum::<usize>();
		if characters < MIN_PAGE_CHARACTERS {
			pages_without_text.push(number);
			continue;
		}
		for raw in page_lines {
			let line = raw
				.chars()
				.map(|c| if c.is_control() { ' ' } else { c })
				.collect::<String>()
				.split_whitespace()
				.collect::<Vec<_>>()
				.join(" ");
			if line.is_empty() {
				continue;
			}
			let (line, cut) = aidash_domain::research::bounded(&line, MAX_LINE_BYTES);
			if lines.len() == MAX_LINES || bytes + line.len() > MAX_SNAPSHOT_BYTES {
				completeness = Completeness::TextLimit;
				break 'pages;
			}
			if cut {
				completeness = Completeness::TextLimit;
			}
			bytes += line.len();
			lines.push(Line {
				text: line.to_owned(),
				page: Some(number),
			});
		}
	}
	Ok(PdfText {
		title: title(&pdf),
		page_count: u32::try_from(pages.len()).map_err(|_| Failure::ExtractionFailed)?,
		lines,
		pages_without_text,
		completeness,
	})
}

/// Render page `page` (1-based) of `pdf` as a PNG within the page image
/// bounds, at a lower scale when the image would be too large. Call it only
/// inside the bounded child.
pub fn render(pdf: &[u8], page: u32) -> Result<RenderedPage, Failure> {
	let pdf = open(pdf)?;
	let pages = pdf.pages();
	let page = usize::try_from(page)
		.ok()
		.and_then(|page| page.checked_sub(1))
		.and_then(|index| pages.get(index))
		.ok_or(Failure::PageNotViewable)?;
	let (width, height) = page.render_dimensions();
	if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
		return Err(Failure::RenderFailed);
	}
	let settings = InterpreterSettings {
		render_annotations: false,
		..InterpreterSettings::default()
	};
	let cache = RenderCache::new();
	let mut scale = (MAX_PAGE_IMAGE_EDGE as f32 - 1.0) / width.max(height);
	for _ in 0..4 {
		let pixmap = hayro::render(
			page,
			&cache,
			&settings,
			&RenderSettings::default(),
			&PixmapSettings {
				x_scale: scale,
				y_scale: scale,
				..PixmapSettings::default()
			},
		);
		let (pixels_wide, pixels_high) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
		let png = pixmap.into_png().map_err(|_| Failure::RenderFailed)?;
		let image = RenderedPage {
			media_type: "image/png".into(),
			bytes: png,
			width: pixels_wide,
			height: pixels_high,
		};
		if image.validate().is_ok() {
			return Ok(image);
		}
		// Too many bytes: a smaller image of the same page.
		scale *= 0.7;
	}
	Err(Failure::RenderFailed)
}

/// Open a PDF that needs no password and permits extracting its text.
fn open(pdf: &[u8]) -> Result<Pdf, Failure> {
	let pdf = Pdf::new(pdf.to_vec()).map_err(|error| match error {
		LoadPdfError::Decryption(_) => Failure::EncryptedDocument,
		LoadPdfError::Invalid => Failure::ExtractionFailed,
	})?;
	if !extraction_permitted(&pdf) {
		return Err(Failure::ExtractionNotPermitted);
	}
	Ok(pdf)
}

/// Whether the standard security handler, if any, permits copying or
/// otherwise extracting text (bit 5 of `P`). Encryption dictionaries are
/// never encrypted themselves, so the dictionary is found among the objects.
fn extraction_permitted(pdf: &Pdf) -> bool {
	const EXTRACT: i64 = 1 << 4;
	pdf.objects()
		.into_iter()
		.filter_map(|object| match object {
			Object::Dict(dict) => Some(dict),
			_ => None,
		})
		.find(is_standard_security)
		.and_then(|dict| dict.get::<i64>(b"P"))
		.is_none_or(|permissions| permissions & EXTRACT != 0)
}

fn is_standard_security(dict: &Dict<'_>) -> bool {
	dict.get::<Name<'_>>(b"Filter")
		.is_some_and(|filter| filter.as_ref() == b"Standard")
		&& dict.contains_key(b"O")
		&& dict.contains_key(b"U")
}

/// The document title from its information dictionary: UTF-16BE with a byte
/// order mark, or otherwise PDFDocEncoding, read as Latin-1.
fn title(pdf: &Pdf) -> Option<String> {
	let raw = pdf.metadata().title.as_ref()?;
	let decoded = match raw.strip_prefix(&[0xfe, 0xff]) {
		Some(utf16) => String::from_utf16_lossy(
			&utf16
				.chunks_exact(2)
				.map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
				.collect::<Vec<_>>(),
		),
		None => raw.iter().map(|byte| char::from(*byte)).collect(),
	};
	let title = decoded
		.chars()
		.filter(|c| !c.is_control())
		.collect::<String>()
		.split_whitespace()
		.collect::<Vec<_>>()
		.join(" ");
	(!title.is_empty()).then(|| aidash_domain::research::bounded(&title, 512).0.to_owned())
}

/// Collects the Unicode text of glyphs into visual lines, in content-stream
/// order. A glyph starts a new line when its baseline moves by more than half
/// its height or it jumps far back to the left, and a space is inserted where
/// the gap after the previous glyph's advance exceeds a fraction of the height,
/// as producers that position words instead of drawing spaces require.
#[derive(Default)]
struct TextDevice {
	lines: Vec<String>,
	current: String,
	/// Baseline, end of the previous glyph's advance, and its height.
	previous: Option<(f64, f64, f64)>,
}

impl TextDevice {
	fn finish(mut self) -> Vec<String> {
		self.end_line();
		self.lines
	}
	fn end_line(&mut self) {
		if !self.current.trim().is_empty() {
			self.lines.push(std::mem::take(&mut self.current));
		}
		self.current.clear();
	}
}

/// The linear part of `transform` applied to a vector.
fn apply(transform: Affine, x: f64, y: f64) -> (f64, f64) {
	let [a, b, c, d, _, _] = transform.as_coeffs();
	(a * x + c * y, b * x + d * y)
}

impl<'a> Device<'a> for TextDevice {
	fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
	fn push_clip_path(&mut self, _: &ClipPath) {}
	fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
	fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
		for glyph in run.glyphs() {
			let transform = props.transform * glyph.transform();
			let origin = transform * Point::ORIGIN;
			// Glyph space has 1000 units per em.
			let (ex, ey) = apply(transform, 0.0, 1000.0);
			let height = ex.hypot(ey).max(f64::EPSILON);
			if let Some((baseline, end, previous_height)) = self.previous {
				let size = height.max(previous_height);
				if (origin.y - baseline).abs() > size / 2.0 || origin.x < end - 2.0 * size {
					self.end_line();
				} else if origin.x - end > size * 0.15 && !self.current.ends_with(' ') {
					self.current.push(' ');
				}
			}
			match glyph.as_unicode() {
				Some(hayro::hayro_interpret::hayro_cmap::BfString::Char(c)) => self.current.push(c),
				Some(hayro::hayro_interpret::hayro_cmap::BfString::String(text)) => {
					self.current.push_str(&text);
				}
				None => {}
			}
			let advance = match &**glyph {
				hayro::hayro_interpret::font::Glyph::Outline(outline) => outline.advance_width(),
				hayro::hayro_interpret::font::Glyph::Type3(_) => None,
			}
			.map_or(height / 2.0, |advance| {
				let (ax, ay) = apply(transform, f64::from(advance), 0.0);
				ax.hypot(ay)
			});
			self.previous = Some((origin.y, origin.x + advance, height));
		}
	}
	fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
	fn pop_clip(&mut self) {}
	fn pop_transparency_group(&mut self) {}
}

#[cfg(test)]
pub(crate) mod tests;
