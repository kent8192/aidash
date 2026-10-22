//! Page reading parses a PDF only in a bounded child of the `aidash` binary
//! (#312). These tests start the real binary as that child.
use aidash_domain::research::{Failure, document::Line};
use aidash_integrations::research::pdf::{ChildEngine, PdfEngine};
use rstest::rstest;
use std::{io::Write, path::PathBuf, time::Duration};
#[cfg(target_os = "linux")]
use tokio::sync::OnceCell;

/// The child engine, after one long-deadline warm-up start on Linux.
async fn engine() -> ChildEngine {
	let program = PathBuf::from(env!("CARGO_BIN_EXE_aidash"));
	#[cfg(target_os = "linux")]
	{
		static WARMED: OnceCell<()> = OnceCell::const_new();
		WARMED
			.get_or_init(|| async {
				let _ = aidash_runtime::child::run(
					&program,
					&["__research-pdf", "extract"],
					b"",
					aidash_runtime::child::Limits {
						cpu_seconds: 120,
						memory_bytes: Some(1 << 30),
						deadline: Duration::from_secs(120),
						max_output: 1 << 20,
					},
				)
				.await;
			})
			.await;
	}
	ChildEngine::new(program)
}

/// Generated with Ghostscript: one line of text, encrypted with an owner
/// password whose permissions allow extraction.
const PERMITTED: &[u8] = include_bytes!(
	"../../../../../crates/aidash-integrations/src/research/pdf/fixtures/extraction-permitted.pdf"
);

#[rstest]
#[tokio::test]
#[cfg(target_os = "linux")]
async fn the_child_extracts_lines_with_their_pages() {
	let text = engine().await.extract(PERMITTED.to_vec()).await.unwrap();
	assert_eq!(
		text.lines,
		vec![Line {
			text: "Protected evidence on page one.".into(),
			page: Some(1),
		}]
	);
}

#[rstest]
#[tokio::test]
#[cfg(target_os = "linux")]
async fn the_child_renders_a_page_without_text_within_the_image_bounds() {
	let report = aidash_integrations::research::fixture::ANNUAL_REPORT_PDF.to_vec();
	let image = PdfEngine::render(&engine().await, report.clone(), 2)
		.await
		.unwrap();
	assert!(image.validate().is_ok());
	assert_eq!(image.media_type, "image/png");
	assert_eq!(
		PdfEngine::render(&engine().await, report, 9).await,
		Err(Failure::PageNotViewable)
	);
}

#[rstest]
#[tokio::test]
#[cfg(target_os = "linux")]
async fn an_unreadable_document_fails_without_harming_the_parent() {
	let outcome = engine()
		.await
		.extract(b"%PDF-1.7 not really".to_vec())
		.await;
	assert_eq!(outcome, Err(Failure::ExtractionFailed));
}

/// A one-page PDF whose content stream inflates to `inflated` zero bytes.
fn decompression_bomb(inflated: usize) -> Vec<u8> {
	let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
	let block = vec![0u8; 1024 * 1024];
	for _ in 0..inflated / block.len() {
		encoder.write_all(&block).unwrap();
	}
	let stream = encoder.finish().unwrap();
	let objects: [Vec<u8>; 4] = [
		b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
		b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
		b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
		[
			format!(
				"<< /Length {} /Filter /FlateDecode >>\nstream\n",
				stream.len()
			)
			.into_bytes(),
			stream,
			b"\nendstream".to_vec(),
		]
		.concat(),
	];
	let mut out = b"%PDF-1.7\n".to_vec();
	let mut offsets = Vec::new();
	for (index, object) in objects.iter().enumerate() {
		offsets.push(out.len());
		out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
		out.extend_from_slice(object);
		out.extend_from_slice(b"\nendobj\n");
	}
	let xref = out.len();
	out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
	for offset in offsets {
		out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
	}
	out.extend_from_slice(
		format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
	);
	out
}

#[rstest]
#[tokio::test]
async fn a_decompression_bomb_is_stopped_by_the_child_limits() {
	// 1.5 GiB inflated from a few MiB: Linux's address-space limit stops it;
	// other platforms refuse the job before parsing. Either way, the read fails
	// cleanly while the parent stays unharmed.
	let bomb = decompression_bomb(1536 * 1024 * 1024);
	assert!(bomb.len() < aidash_integrations::research::pdf::MAX_PDF_BYTES);
	let started = std::time::Instant::now();
	let outcome = engine().await.extract(bomb).await;
	assert_eq!(outcome, Err(Failure::ExtractionFailed));
	assert!(started.elapsed() < Duration::from_secs(40));
}

#[cfg(not(target_os = "linux"))]
#[rstest]
#[tokio::test]
async fn ordinary_pdf_jobs_fail_closed_without_memory_containment() {
	let engine = engine().await;
	assert_eq!(
		engine.extract(PERMITTED.to_vec()).await,
		Err(Failure::ExtractionFailed)
	);
	assert_eq!(
		PdfEngine::render(
			&engine,
			aidash_integrations::research::fixture::ANNUAL_REPORT_PDF.to_vec(),
			2,
		)
		.await,
		Err(Failure::RenderFailed)
	);
}
