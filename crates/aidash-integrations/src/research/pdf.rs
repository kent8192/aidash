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
		MAX_PAGES_WITHOUT_TEXT, MAX_SNAPSHOT_BYTES, RenderedPage,
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
		reader::{Reader, ReaderExt},
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
// Every page read may carry no text, and a snapshot lists each such page.
const _: () = assert!(MAX_PAGES <= MAX_PAGES_WITHOUT_TEXT);
/// A page with fewer non-whitespace characters of text carries no text.
pub const MIN_PAGE_CHARACTERS: usize = 16;
/// The hidden command of the `aidash` binary that runs one PDF job.
pub const CHILD_COMMAND: &str = "__research-pdf";
/// The longest reply of an extraction within the snapshot bounds: every
/// text byte JSON-escaped (at most six bytes, as `\u001f`), the object around
/// each line, every page listed without text, a title and the envelope.
const MAX_REPLY_BYTES: usize = 6 * (MAX_SNAPSHOT_BYTES + MAX_TITLE_BYTES)
	+ MAX_LINES * LINE_ENVELOPE_BYTES
	+ MAX_PAGES * 12
	+ 4 * 1024;
/// The longest title a snapshot keeps, in bytes.
const MAX_TITLE_BYTES: usize = 512;
/// `{"text":"","page":4294967295},` around one line's text.
const LINE_ENVELOPE_BYTES: usize = 32;

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
				memory_bytes: Some(1024 * 1024 * 1024),
				deadline: Duration::from_secs(20),
				// Room for the text reply or a base64 page image.
				max_output: MAX_REPLY_BYTES.max(2 * MAX_PAGE_IMAGE_BYTES + 64 * 1024),
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
		let Some(page_lines) = page_text(device.finish()) else {
			pages_without_text.push(number);
			continue;
		};
		for line in page_lines {
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

/// A page's lines as a snapshot keeps them, or `None` when that text has
/// fewer than [`MIN_PAGE_CHARACTERS`] non-whitespace characters. Control
/// characters become spaces first, so they never make a page text-bearing.
fn page_text(raw: Vec<String>) -> Option<Vec<String>> {
	let lines = raw
		.into_iter()
		.map(|line| {
			line.chars()
				.map(|c| if c.is_control() { ' ' } else { c })
				.collect::<String>()
				.split_whitespace()
				.collect::<Vec<_>>()
				.join(" ")
		})
		.filter(|line| !line.is_empty())
		.collect::<Vec<_>>();
	let characters = lines
		.iter()
		.map(|line| line.chars().filter(|c| !c.is_whitespace()).count())
		.sum::<usize>();
	(characters >= MIN_PAGE_CHARACTERS).then_some(lines)
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
/// never encrypted themselves, so they are found among the objects, including
/// the trailer dictionaries of cross-reference streams.
///
/// hayro does not expose which trailer names the effective dictionary, so
/// every standard security dictionary in the file must permit extraction: a
/// decoy that permits it cannot outvote the real one, and a forbidding decoy
/// only refuses its own document. A file whose trailer names an encryption
/// dictionary none of which is found, such as one written directly into a
/// classic trailer, is refused rather than read.
fn extraction_permitted(pdf: &Pdf) -> bool {
	const EXTRACT: i64 = 1 << 4;
	let mut found = false;
	for object in pdf.objects() {
		let dict = match object {
			Object::Dict(dict) => Some(dict),
			Object::Stream(stream) => stream.dict().get::<Dict<'_>>(b"Encrypt"),
			_ => None,
		};
		let Some(dict) = dict.filter(is_standard_security) else {
			continue;
		};
		found = true;
		if dict
			.get::<i64>(b"P")
			.is_some_and(|permissions| permissions & EXTRACT == 0)
		{
			return false;
		}
	}
	found || !names_encryption(pdf)
}

/// Whether a trailer names an encryption dictionary: a classic `trailer`
/// dictionary, or the dictionary of a cross-reference stream, with an
/// `Encrypt` entry. The bytes `/Encrypt` anywhere else, such as in a content
/// stream or metadata, name nothing.
fn names_encryption(pdf: &Pdf) -> bool {
	const TRAILER: &[u8] = b"trailer";
	let data = pdf.data().as_ref();
	let classic = data
		.windows(TRAILER.len())
		.enumerate()
		.filter(|(_, window)| *window == TRAILER)
		.any(|(at, _)| {
			let mut reader = Reader::new(data);
			reader.jump(at + TRAILER.len());
			reader.skip_white_spaces_and_comments();
			reader
				.read_without_context::<Dict<'_>>()
				.is_some_and(|trailer| trailer.contains_key(b"Encrypt"))
		});
	classic
		|| pdf.objects().into_iter().any(|object| match object {
			Object::Stream(stream) => {
				let dict = stream.dict();
				dict.get::<Name<'_>>(b"Type")
					.is_some_and(|kind| kind.as_ref() == b"XRef")
					&& dict.contains_key(b"Encrypt")
			}
			_ => false,
		})
}

fn is_standard_security(dict: &Dict<'_>) -> bool {
	dict.get::<Name<'_>>(b"Filter")
		.is_some_and(|filter| filter.as_ref() == b"Standard")
		&& dict.contains_key(b"O")
		&& dict.contains_key(b"U")
}

/// The document title from its information dictionary: UTF-16BE or (PDF 2.0)
/// UTF-8 with a byte order mark, or otherwise PDFDocEncoding.
fn title(pdf: &Pdf) -> Option<String> {
	normalized_title(&decode_text_string(pdf.metadata().title.as_ref()?))
}

/// A PDF text string (ISO 32000-2, 7.9.2.2) as Unicode.
fn decode_text_string(raw: &[u8]) -> String {
	if let Some(utf16) = raw.strip_prefix(&[0xfe, 0xff]) {
		return String::from_utf16_lossy(
			&utf16
				.chunks_exact(2)
				.map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
				.collect::<Vec<_>>(),
		);
	}
	if let Some(utf8) = raw.strip_prefix(&[0xef, 0xbb, 0xbf]) {
		return String::from_utf8_lossy(utf8).into_owned();
	}
	raw.iter().filter_map(|byte| pdf_doc_char(*byte)).collect()
}

/// One PDFDocEncoding byte (ISO 32000-2, Annex D.2), or none for the bytes it
/// leaves undefined. It matches Latin-1 except where listed here.
fn pdf_doc_char(byte: u8) -> Option<char> {
	Some(match byte {
		0x18 => '\u{02D8}',
		0x19 => '\u{02C7}',
		0x1A => '\u{02C6}',
		0x1B => '\u{02D9}',
		0x1C => '\u{02DD}',
		0x1D => '\u{02DB}',
		0x1E => '\u{02DA}',
		0x1F => '\u{02DC}',
		0x80 => '\u{2022}',
		0x81 => '\u{2020}',
		0x82 => '\u{2021}',
		0x83 => '\u{2026}',
		0x84 => '\u{2014}',
		0x85 => '\u{2013}',
		0x86 => '\u{0192}',
		0x87 => '\u{2044}',
		0x88 => '\u{2039}',
		0x89 => '\u{203A}',
		0x8A => '\u{2212}',
		0x8B => '\u{2030}',
		0x8C => '\u{201E}',
		0x8D => '\u{201C}',
		0x8E => '\u{201D}',
		0x8F => '\u{2018}',
		0x90 => '\u{2019}',
		0x91 => '\u{201A}',
		0x92 => '\u{2122}',
		0x93 => '\u{FB01}',
		0x94 => '\u{FB02}',
		0x95 => '\u{0141}',
		0x96 => '\u{0152}',
		0x97 => '\u{0160}',
		0x98 => '\u{0178}',
		0x99 => '\u{017D}',
		0x9A => '\u{0131}',
		0x9B => '\u{0142}',
		0x9C => '\u{0153}',
		0x9D => '\u{0161}',
		0x9E => '\u{017E}',
		0xA0 => '\u{20AC}',
		0x7F | 0x9F | 0xAD => return None,
		other => char::from(other),
	})
}

/// A title with control characters removed and whitespace collapsed, within
/// the snapshot's title bound.
fn normalized_title(decoded: &str) -> Option<String> {
	let title = decoded
		.chars()
		.filter(|c| !c.is_control())
		.collect::<String>()
		.split_whitespace()
		.collect::<Vec<_>>()
		.join(" ");
	(!title.is_empty()).then(|| {
		aidash_domain::research::bounded(&title, MAX_TITLE_BYTES)
			.0
			.to_owned()
	})
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
