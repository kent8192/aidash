//! Bounded decoded validation of media supplied as Thread Attachments.
use std::io::Cursor;
use symphonia::core::{
	codecs::{CODEC_TYPE_OPUS, CodecParameters, Decoder, DecoderOptions},
	errors::Error as AudioError,
	formats::FormatOptions,
	io::{MediaSourceStream, MediaSourceStreamOptions},
	meta::MetadataOptions,
	probe::Hint,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Measurements {
	pub width: Option<i32>,
	pub height: Option<i32>,
	pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
	Undecodable,
	TooLarge,
}

pub fn validate(media_type: &str, bytes: &[u8]) -> Result<Measurements, Refusal> {
	if (media_type.starts_with("image/") || media_type.starts_with("audio/"))
		&& aidash_domain::provider::ContentPart::from_media(media_type, bytes.to_vec()).is_err()
	{
		return Err(Refusal::Undecodable);
	}
	if media_type.starts_with("image/") {
		return image(media_type, bytes);
	}
	if media_type.starts_with("audio/") {
		return audio(media_type, bytes);
	}
	Ok(Measurements::default())
}

fn image(media_type: &str, bytes: &[u8]) -> Result<Measurements, Refusal> {
	let format = match media_type {
		"image/png" => image::ImageFormat::Png,
		"image/jpeg" => image::ImageFormat::Jpeg,
		"image/gif" => image::ImageFormat::Gif,
		"image/webp" => image::ImageFormat::WebP,
		_ => return Err(Refusal::Undecodable),
	};
	if image::guess_format(bytes).map_err(|_| Refusal::Undecodable)? != format {
		return Err(Refusal::Undecodable);
	}
	let reader = image::ImageReader::with_format(Cursor::new(bytes), format);
	let (width, height) = reader.into_dimensions().map_err(|_| Refusal::Undecodable)?;
	if u64::from(width) * u64::from(height) > 40_000_000 {
		return Err(Refusal::TooLarge);
	}
	image::load_from_memory_with_format(bytes, format).map_err(|_| Refusal::Undecodable)?;
	Ok(Measurements {
		width: Some(i32::try_from(width).map_err(|_| Refusal::TooLarge)?),
		height: Some(i32::try_from(height).map_err(|_| Refusal::TooLarge)?),
		duration_ms: None,
	})
}

enum AudioDecoder {
	Symphonia(Box<dyn Decoder>),
	Opus(Box<opus_rs::OpusDecoder>, usize),
	OpusMulti(Box<opus_rs::multistream::MultistreamDecoder>, usize),
}

impl AudioDecoder {
	fn new(params: &CodecParameters) -> Result<Self, Refusal> {
		if params.codec != CODEC_TYPE_OPUS {
			return symphonia::default::get_codecs()
				.make(params, &DecoderOptions::default())
				.map(Self::Symphonia)
				.map_err(|_| Refusal::Undecodable);
		}
		let header = params.extra_data.as_deref().ok_or(Refusal::Undecodable)?;
		if header.len() < 19 || &header[..8] != b"OpusHead" {
			return Err(Refusal::Undecodable);
		}
		let channels = usize::from(header[9]);
		if !(1..=8).contains(&channels) {
			return Err(Refusal::Undecodable);
		}
		match header[18] {
			0 if channels <= 2 => Ok(Self::Opus(
				Box::new(
					opus_rs::OpusDecoder::new(48_000, channels)
						.map_err(|_| Refusal::Undecodable)?,
				),
				channels,
			)),
			1 => {
				let table = opus_rs::multistream::ChannelMappingTable::parse(
					1,
					channels as u8,
					&header[19..],
				)
				.ok_or(Refusal::Undecodable)?;
				let decoded_channels =
					usize::from(table.stream_count) + usize::from(table.coupled_count);
				if table.stream_count == 0
					|| table.stream_count as usize > channels
					|| table.coupled_count > table.stream_count
					|| decoded_channels > channels
					|| table
						.mapping
						.iter()
						.any(|&c| c != 255 && usize::from(c) >= decoded_channels)
				{
					return Err(Refusal::Undecodable);
				}
				Ok(Self::OpusMulti(
					Box::new(
						opus_rs::multistream::MultistreamDecoder::new(48_000, table)
							.map_err(|_| Refusal::Undecodable)?,
					),
					channels,
				))
			}
			_ => Err(Refusal::Undecodable),
		}
	}
	fn frames(&mut self, packet: &symphonia::core::formats::Packet) -> Result<(u64, u32), Refusal> {
		match self {
			Self::Symphonia(decoder) => {
				let decoded = decoder.decode(packet).map_err(|_| Refusal::Undecodable)?;
				Ok((decoded.frames() as u64, decoded.spec().rate))
			}
			Self::Opus(decoder, channels) => {
				let mut samples = vec![0.0; 5760 * *channels];
				let frames = decoder
					.decode(packet.buf(), 5760, &mut samples)
					.map_err(|_| Refusal::Undecodable)?;
				let trimmed = frames
					.checked_sub(packet.trim_start() as usize + packet.trim_end() as usize)
					.ok_or(Refusal::Undecodable)?;
				Ok((trimmed as u64, 48_000))
			}
			Self::OpusMulti(decoder, channels) => {
				let mut samples = vec![0.0; 5760 * *channels];
				let frames = decoder
					.decode(packet.buf(), 5760, &mut samples)
					.map_err(|_| Refusal::Undecodable)?;
				let trimmed = frames
					.checked_sub(packet.trim_start() as usize + packet.trim_end() as usize)
					.ok_or(Refusal::Undecodable)?;
				Ok((trimmed as u64, 48_000))
			}
		}
	}
}

fn audio(media_type: &str, bytes: &[u8]) -> Result<Measurements, Refusal> {
	if matches!(media_type, "audio/wav" | "audio/x-wav") {
		let declared = bytes
			.get(4..8)
			.and_then(|value| <[u8; 4]>::try_from(value).ok())
			.map(u32::from_le_bytes)
			.ok_or(Refusal::Undecodable)?;
		if u64::from(declared) + 8 > bytes.len() as u64 {
			return Err(Refusal::Undecodable);
		}
	}
	let extension = match media_type {
		"audio/wav" | "audio/x-wav" => "wav",
		"audio/mpeg" => "mp3",
		"audio/mp4" | "audio/x-m4a" => "m4a",
		"audio/aac" => "aac",
		"audio/ogg" => "ogg",
		"audio/webm" => "webm",
		"audio/flac" | "audio/x-flac" => "flac",
		_ => return Err(Refusal::Undecodable),
	};
	let source = MediaSourceStream::new(
		Box::new(Cursor::new(bytes.to_vec())),
		MediaSourceStreamOptions::default(),
	);
	let mut hint = Hint::new();
	hint.with_extension(extension);
	let mut format = symphonia::default::get_probe()
		.format(
			&hint,
			source,
			&FormatOptions {
				enable_gapless: true,
				..Default::default()
			},
			&MetadataOptions::default(),
		)
		.map_err(|_| Refusal::Undecodable)?
		.format;
	let track = format.default_track().ok_or(Refusal::Undecodable)?;
	let id = track.id;
	let mut decoder = AudioDecoder::new(&track.codec_params)?;
	let mut nanos = 0_u128;
	let mut frames = 0_u64;
	loop {
		let packet = match format.next_packet() {
			Ok(packet) => packet,
			Err(AudioError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
			Err(_) => return Err(Refusal::Undecodable),
		};
		if packet.track_id() != id {
			continue;
		}
		let (count, sample_rate) = decoder.frames(&packet)?;
		if sample_rate == 0 {
			return Err(Refusal::Undecodable);
		}
		frames = frames.checked_add(count).ok_or(Refusal::TooLarge)?;
		nanos = nanos
			.checked_add(u128::from(count) * 1_000_000_000 / u128::from(sample_rate))
			.ok_or(Refusal::TooLarge)?;
		if nanos > 600_000_000_000 {
			return Err(Refusal::TooLarge);
		}
	}
	if frames == 0 {
		return Err(Refusal::Undecodable);
	}
	Ok(Measurements {
		width: None,
		height: None,
		duration_ms: Some(((nanos + 999_999) / 1_000_000) as i64),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn pcm_wav(seconds: u32) -> Vec<u8> {
		let data = seconds * 8_000;
		let mut bytes = vec![0; 44 + data as usize];
		bytes[..4].copy_from_slice(b"RIFF");
		bytes[4..8].copy_from_slice(&(36 + data).to_le_bytes());
		bytes[8..12].copy_from_slice(b"WAVE");
		bytes[12..16].copy_from_slice(b"fmt ");
		bytes[16..20].copy_from_slice(&16_u32.to_le_bytes());
		bytes[20..22].copy_from_slice(&1_u16.to_le_bytes());
		bytes[22..24].copy_from_slice(&1_u16.to_le_bytes());
		bytes[24..28].copy_from_slice(&8_000_u32.to_le_bytes());
		bytes[28..32].copy_from_slice(&8_000_u32.to_le_bytes());
		bytes[32..34].copy_from_slice(&1_u16.to_le_bytes());
		bytes[34..36].copy_from_slice(&8_u16.to_le_bytes());
		bytes[36..40].copy_from_slice(b"data");
		bytes[40..44].copy_from_slice(&data.to_le_bytes());
		bytes
	}

	#[test]
	fn decodes_supported_audio_containers() {
		for (mime, bytes) in [
			(
				"audio/aac",
				include_bytes!("media/fixtures/short.aac").as_slice(),
			),
			(
				"audio/mp4",
				include_bytes!("media/fixtures/short.m4a").as_slice(),
			),
			(
				"audio/mpeg",
				include_bytes!("media/fixtures/short.mp3").as_slice(),
			),
			(
				"audio/flac",
				include_bytes!("media/fixtures/short.flac").as_slice(),
			),
			(
				"audio/ogg",
				include_bytes!("media/fixtures/short.ogg").as_slice(),
			),
		] {
			let measured = validate(mime, bytes).unwrap_or_else(|e| panic!("{mime}: {e:?}"));
			assert!(measured.duration_ms.unwrap() > 0, "{mime}: {measured:?}");
		}
	}

	#[test]
	fn decodes_opus_in_ogg_and_webm() {
		for (mime, bytes) in [
			(
				"audio/ogg",
				include_bytes!("media/fixtures/short-opus.ogg").as_slice(),
			),
			(
				"audio/webm",
				include_bytes!("media/fixtures/short-opus.webm").as_slice(),
			),
		] {
			let measured = validate(mime, bytes).unwrap();
			assert!(
				(90..=120).contains(&measured.duration_ms.unwrap()),
				"{mime}: {measured:?}"
			);
		}
	}

	#[test]
	fn decoded_images_record_dimensions_and_reject_wrong_or_corrupt_content() {
		for (mime, format) in [
			("image/png", image::ImageFormat::Png),
			("image/jpeg", image::ImageFormat::Jpeg),
			("image/gif", image::ImageFormat::Gif),
			("image/webp", image::ImageFormat::WebP),
		] {
			let mut bytes = Cursor::new(Vec::new());
			image::DynamicImage::new_rgb8(2, 3)
				.write_to(&mut bytes, format)
				.unwrap();
			assert_eq!(
				validate(mime, bytes.get_ref()),
				Ok(Measurements {
					width: Some(2),
					height: Some(3),
					duration_ms: None
				})
			);
			assert_eq!(
				validate("image/png", b"\x89PNG\r\n\x1a\ntruncated"),
				Err(Refusal::Undecodable)
			);
		}
		let oversized = [
			0x89, 0x50, 0x4e, 0x47, 0xd, 0xa, 0x1a, 0xa, 0, 0, 0, 0xd, 0x49, 0x48, 0x44, 0x52, 0,
			0, 0x27, 0x10, 0, 0, 0xf, 0xa1, 8, 2, 0, 0, 0, 0x18, 0xe8, 0x42, 0x5b, 0, 0, 0, 0xc,
			0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0, 0, 3, 1, 1, 0, 0xc9,
			0xfe, 0x92, 0xef, 0, 0, 0, 0, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
		];
		assert_eq!(validate("image/png", &oversized), Err(Refusal::TooLarge));
	}

	#[test]
	fn decoded_audio_measures_samples_and_enforces_ten_minutes() {
		assert_eq!(
			validate("audio/wav", &pcm_wav(1)),
			Ok(Measurements {
				width: None,
				height: None,
				duration_ms: Some(1000)
			})
		);
		assert_eq!(
			validate("audio/wav", &pcm_wav(600)).unwrap().duration_ms,
			Some(600_000)
		);
		assert_eq!(validate("audio/wav", &pcm_wav(601)), Err(Refusal::TooLarge));
		assert_eq!(
			validate("audio/wav", b"RIFF\0\0\0\0WAVEbad"),
			Err(Refusal::Undecodable)
		);
		assert_eq!(
			validate("audio/mpeg", &pcm_wav(1)),
			Err(Refusal::Undecodable)
		);
	}
}
