//! Raw JPEG frames from the verified Previewer profile, with one bounded unread frame.
//! Complete JPEG validation belongs to the native packet parser; the mailbox checks
//! metadata, payload bounds and stream identity without decoding each image twice.
#[cfg(windows)]
use crate::ipc::model::NativeFrame;
use crate::ipc::model::{BinaryFrameMetadata, NativeFrameFormat};
use crate::scheduler::{BuildTask, FrameReject};
#[cfg(windows)]
use base64::{engine::general_purpose::STANDARD, Engine};
#[cfg(windows)]
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
#[cfg(windows)]
use std::io::Cursor;

pub const BINARY_FRAME_PROTOCOL_VERSION: u32 = 1;
pub const JPEG40_HEADER_BYTES: usize = 40;
pub const MAX_JPEG40_PACKET_BYTES: usize = 1024 * 1024;
pub const MAX_JPEG_PAYLOAD_BYTES: usize = MAX_JPEG40_PACKET_BYTES - JPEG40_HEADER_BYTES;
pub const MAX_BINARY_METADATA_BYTES: usize = 4096;
const SURFACE_WIDTH: u32 = 360;
const SURFACE_HEIGHT: u32 = 640;
pub const MAX_SURFACE_DIMENSION: u32 = 3000;
pub const MAX_SURFACE_PIXELS: u64 = 4 * 1024 * 1024;
const MAX_ID_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryFrame {
    pub metadata: BinaryFrameMetadata,
    pub payload: Vec<u8>,
}

/// This checks the binary envelope. `parse_jpeg40` additionally decodes the
/// complete JPEG with allocation and dimension limits before publishing it.
pub fn validate_binary_frame(frame: &BinaryFrame) -> Result<(), FrameReject> {
    if frame.metadata.width != SURFACE_WIDTH || frame.metadata.height != SURFACE_HEIGHT {
        return Err(FrameReject::InvalidDimensions);
    }
    validate_binary_frame_envelope(frame)
}

/// Dynamic dimensions are admitted by the control worker against its expected
/// native surface before reaching this bounded shared transport validation.
pub fn validate_binary_frame_envelope(frame: &BinaryFrame) -> Result<(), FrameReject> {
    let metadata = &frame.metadata;
    if metadata.protocol_version != BINARY_FRAME_PROTOCOL_VERSION
        || [
            &metadata.workspace_id,
            &metadata.session_id,
            &metadata.build_id,
        ]
        .iter()
        .any(|id| id.is_empty() || id.len() > MAX_ID_BYTES)
        || metadata.generation == 0
        || metadata.sequence == 0
        || metadata.payload_length as usize != frame.payload.len()
    {
        return Err(FrameReject::InvalidMetadata);
    }
    if !(50..=MAX_SURFACE_DIMENSION).contains(&metadata.width)
        || !(50..=MAX_SURFACE_DIMENSION).contains(&metadata.height)
        || u64::from(metadata.width) * u64::from(metadata.height) > MAX_SURFACE_PIXELS {
        return Err(FrameReject::InvalidDimensions);
    }
    if metadata.rotation != 0 {
        return Err(FrameReject::InvalidRotation);
    }
    if metadata.format != NativeFrameFormat::Jpeg {
        return Err(FrameReject::InvalidEncoding);
    }
    if frame.payload.len() > MAX_JPEG_PAYLOAD_BYTES {
        return Err(FrameReject::PayloadTooLarge);
    }
    if frame.payload.len() < 4
        || !frame.payload.starts_with(&[0xff, 0xd8])
        || !frame.payload.ends_with(&[0xff, 0xd9])
    {
        return Err(FrameReject::InvalidEncoding);
    }
    Ok(())
}

/// Only the exact 40-byte big-endian header verified for SDK 26.0.0.105 is accepted.
/// The original compressed image bytes remain unchanged after bounded validation.
#[cfg(windows)]
pub fn parse_jpeg40(
    data: &[u8],
    task: &BuildTask,
    sequence: u64,
    captured_at: u64,
) -> Result<BinaryFrame, String> {
    parse_jpeg40_for_surface(data, task, sequence, captured_at, SURFACE_WIDTH, SURFACE_HEIGHT)
}

/// Same verified JPEG40 header, matched to a Rust-admitted native surface.
/// This does not accept arbitrary header variants or rotate pixels in CSS.
#[cfg(windows)]
pub fn parse_jpeg40_for_surface(
    data: &[u8], task: &BuildTask, sequence: u64, captured_at: u64,
    expected_width: u32, expected_height: u32,
) -> Result<BinaryFrame, String> {
    if data.len() < JPEG40_HEADER_BYTES + 4
        || data.len() > MAX_JPEG40_PACKET_BYTES
        || data[..4] != [0x12, 0x34, 0x56, 0x78]
    {
        return Err("FRAME_HEADER".into());
    }
    if !valid_task_identity(task) || sequence == 0 {
        return Err("FRAME_PAYLOAD: InvalidMetadata".into());
    }
    let width = u32::from_be_bytes(data[4..8].try_into().map_err(|_| "FRAME_WIDTH")?);
    let height = u32::from_be_bytes(data[8..12].try_into().map_err(|_| "FRAME_HEIGHT")?);
    if width != expected_width
        || height != expected_height
        || data[12..20] != data[4..12]
        || data[20..JPEG40_HEADER_BYTES].iter().any(|&byte| byte != 0)
    {
        return Err("FRAME_PROFILE_MISMATCH".into());
    }
    let payload = &data[JPEG40_HEADER_BYTES..];
    let frame = BinaryFrame {
        metadata: BinaryFrameMetadata {
            protocol_version: BINARY_FRAME_PROTOCOL_VERSION,
            workspace_id: task.workspace_id.clone(),
            session_id: task.session_id.clone(),
            build_id: task.build_id.clone(),
            generation: task.generation,
            sequence,
            width,
            height,
            captured_at,
            rotation: 0,
            format: NativeFrameFormat::Jpeg,
            payload_length: payload.len() as u32,
        },
        payload: payload.to_vec(),
    };
    validate_binary_frame_envelope(&frame).map_err(|error| format!("FRAME_PAYLOAD: {error:?}"))?;
    decode_jpeg(&frame)?;
    Ok(frame)
}

#[cfg(windows)]
fn decode_jpeg(frame: &BinaryFrame) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::with_format(Cursor::new(&frame.payload), ImageFormat::Jpeg);
    let mut limits = Limits::default();
    limits.max_image_width = Some(frame.metadata.width);
    limits.max_image_height = Some(frame.metadata.height);
    limits.max_alloc = Some(16 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("FRAME_DECODE: {error}"))?;
    if image.width() != frame.metadata.width || image.height() != frame.metadata.height {
        return Err("FRAME_DIMENSIONS_MISMATCH".into());
    }
    Ok(image)
}

impl BinaryFrame {
    /// Wire format: a 4-byte little-endian JSON length, UTF-8 metadata, raw JPEG.
    /// The metadata and image have independent limits before the packet is allocated.
    pub fn packet(&self) -> Result<Vec<u8>, String> {
        validate_binary_frame_envelope(self).map_err(|error| format!("FRAME_PAYLOAD: {error:?}"))?;
        let metadata = serde_json::to_vec(&self.metadata)
            .map_err(|error| format!("FRAME_METADATA: {error}"))?;
        if metadata.len() > MAX_BINARY_METADATA_BYTES {
            return Err("FRAME_METADATA_LIMIT".into());
        }
        let mut packet = Vec::with_capacity(4 + metadata.len() + self.payload.len());
        packet.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        packet.extend_from_slice(&metadata);
        packet.extend_from_slice(&self.payload);
        Ok(packet)
    }

    /// Explicit compatibility conversion for the existing frontend contract.
    /// Native collection and the binary transport never call this automatically.
    #[cfg(windows)]
    pub fn legacy_png(&self) -> Result<NativeFrame, String> {
        validate_binary_frame_envelope(self).map_err(|error| format!("FRAME_PAYLOAD: {error:?}"))?;
        let image = decode_jpeg(self)?;
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, ImageFormat::Png)
            .map_err(|error| format!("FRAME_ENCODE: {error}"))?;
        if png.get_ref().len() > MAX_JPEG40_PACKET_BYTES {
            return Err("FRAME_PNG_LIMIT".into());
        }
        Ok(NativeFrame {
            session_id: self.metadata.session_id.clone(),
            build_id: self.metadata.build_id.clone(),
            generation: self.metadata.generation,
            sequence: self.metadata.sequence,
            width: self.metadata.width,
            height: self.metadata.height,
            captured_at: self.metadata.captured_at,
            rotation: self.metadata.rotation,
            png_base64: STANDARD.encode(png.into_inner()),
        })
    }
}

/// Counters saturate instead of wrapping. `clear` retains evidence, while
/// `set_stream` starts a fresh measurement period for the new build stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BinaryFrameMetrics {
    pub received: u64,
    pub dropped: u64,
    pub rejected: u64,
    pub bytes_received: u64,
    pub buffered_bytes: usize,
}

#[derive(Default)]
pub struct BinaryFrameMailbox {
    stream: Option<BuildTask>,
    latest: Option<BinaryFrame>,
    last_sequence: Option<u64>,
    metrics: BinaryFrameMetrics,
}

impl BinaryFrameMailbox {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_stream(&mut self, task: &BuildTask) {
        self.clear();
        self.metrics = BinaryFrameMetrics::default();
        if !valid_task_identity(task) {
            return;
        }
        self.stream = Some(task.clone());
    }

    pub fn push(&mut self, frame: BinaryFrame) -> Result<(), FrameReject> {
        if let Err(error) = self.check(&frame) {
            self.metrics.rejected = self.metrics.rejected.saturating_add(1);
            return Err(error);
        }
        self.last_sequence = Some(frame.metadata.sequence);
        self.metrics.received = self.metrics.received.saturating_add(1);
        self.metrics.bytes_received = self
            .metrics
            .bytes_received
            .saturating_add(frame.payload.len() as u64);
        if self.latest.is_some() {
            self.metrics.dropped = self.metrics.dropped.saturating_add(1);
        }
        self.metrics.buffered_bytes = frame.payload.len();
        self.latest = Some(frame);
        Ok(())
    }

    fn check(&self, frame: &BinaryFrame) -> Result<(), FrameReject> {
        let task = self.stream.as_ref().ok_or(FrameReject::Stopped)?;
        if frame.metadata.workspace_id != task.workspace_id
            || frame.metadata.session_id != task.session_id
        {
            return Err(FrameReject::WrongSession);
        }
        if frame.metadata.build_id != task.build_id {
            return Err(FrameReject::WrongBuild);
        }
        if frame.metadata.generation != task.generation {
            return Err(FrameReject::WrongGeneration);
        }
        if self
            .last_sequence
            .is_some_and(|sequence| frame.metadata.sequence <= sequence)
        {
            return Err(FrameReject::OldSequence);
        }
        validate_binary_frame_envelope(frame)
    }

    pub fn take(&mut self) -> Option<BinaryFrame> {
        let frame = self.latest.take();
        self.metrics.buffered_bytes = 0;
        frame
    }

    pub fn clear(&mut self) {
        self.stream = None;
        self.latest = None;
        self.last_sequence = None;
        self.metrics.buffered_bytes = 0;
    }

    pub fn metrics(&self) -> BinaryFrameMetrics {
        self.metrics
    }
}

fn valid_task_identity(task: &BuildTask) -> bool {
    task.generation > 0
        && [&task.workspace_id, &task.session_id, &task.build_id]
            .iter()
            .all(|id| !id.is_empty() && id.len() <= MAX_ID_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> BuildTask {
        BuildTask {
            epoch: 1,
            workspace_id: "test-only-workspace".into(),
            session_id: "test-only-session".into(),
            generation: 1,
            build_id: "test-only-build".into(),
            started_at: 0,
        }
    }

    // This envelope is solely a mailbox test fixture, never native-preview evidence.
    fn envelope(sequence: u64) -> BinaryFrame {
        let task = task();
        BinaryFrame {
            metadata: BinaryFrameMetadata {
                protocol_version: BINARY_FRAME_PROTOCOL_VERSION,
                workspace_id: task.workspace_id,
                session_id: task.session_id,
                build_id: task.build_id,
                generation: task.generation,
                sequence,
                width: SURFACE_WIDTH,
                height: SURFACE_HEIGHT,
                captured_at: 1,
                rotation: 0,
                format: NativeFrameFormat::Jpeg,
                payload_length: 4,
            },
            payload: vec![0xff, 0xd8, 0xff, 0xd9],
        }
    }

    #[test]
    fn slow_consumer_receives_only_newest_frame_with_bounded_metrics() {
        let mut mailbox = BinaryFrameMailbox::new();
        mailbox.set_stream(&task());
        for sequence in 1..=100 {
            mailbox.push(envelope(sequence)).unwrap();
            assert_eq!(mailbox.metrics().buffered_bytes, 4);
        }
        assert_eq!(mailbox.metrics().received, 100);
        assert_eq!(mailbox.metrics().dropped, 99);
        assert_eq!(mailbox.metrics().bytes_received, 400);
        assert_eq!(mailbox.take().unwrap().metadata.sequence, 100);
        assert_eq!(mailbox.metrics().buffered_bytes, 0);
        assert!(mailbox.take().is_none());
    }

    #[test]
    fn old_stream_and_out_of_order_frames_never_replace_the_latest() {
        let mut mailbox = BinaryFrameMailbox::new();
        mailbox.set_stream(&task());
        mailbox.push(envelope(2)).unwrap();
        for field in ["workspace", "session", "build", "generation"] {
            let mut frame = envelope(3);
            let expected = match field {
                "workspace" => {
                    frame.metadata.workspace_id = "old-workspace".into();
                    FrameReject::WrongSession
                }
                "session" => {
                    frame.metadata.session_id = "old-session".into();
                    FrameReject::WrongSession
                }
                "build" => {
                    frame.metadata.build_id = "old-build".into();
                    FrameReject::WrongBuild
                }
                _ => {
                    frame.metadata.generation += 1;
                    FrameReject::WrongGeneration
                }
            };
            assert_eq!(mailbox.push(frame), Err(expected));
        }
        assert_eq!(mailbox.push(envelope(1)), Err(FrameReject::OldSequence));
        assert_eq!(mailbox.push(envelope(2)), Err(FrameReject::OldSequence));
        assert_eq!(mailbox.take().unwrap().metadata.sequence, 2);
        assert_eq!(mailbox.metrics().received, 1);
        assert_eq!(mailbox.metrics().dropped, 0);
        assert_eq!(mailbox.metrics().rejected, 6);
        assert_eq!(mailbox.push(envelope(2)), Err(FrameReject::OldSequence));
    }

    #[test]
    fn stop_discards_pending_pixels_and_rejects_late_frames() {
        let mut mailbox = BinaryFrameMailbox::new();
        mailbox.set_stream(&task());
        mailbox.push(envelope(1)).unwrap();
        mailbox.clear();
        assert!(mailbox.take().is_none());
        assert_eq!(mailbox.push(envelope(2)), Err(FrameReject::Stopped));
        assert_eq!(mailbox.metrics().buffered_bytes, 0);
        assert_eq!(mailbox.metrics().received, 1);
        mailbox.set_stream(&task());
        assert_eq!(mailbox.metrics(), BinaryFrameMetrics::default());
        mailbox.push(envelope(1)).unwrap();
    }

    #[test]
    fn invalid_payload_does_not_consume_its_sequence() {
        let mut mailbox = BinaryFrameMailbox::new();
        mailbox.set_stream(&task());
        let mut malformed = envelope(1);
        malformed.metadata.payload_length += 1;
        assert_eq!(mailbox.push(malformed), Err(FrameReject::InvalidMetadata));
        mailbox.push(envelope(1)).unwrap();
        assert_eq!(mailbox.metrics().received, 1);
        assert_eq!(mailbox.metrics().rejected, 1);
    }

    #[test]
    fn envelope_limits_reject_invalid_version_ids_dimensions_rotation_and_lengths() {
        let valid = envelope(1);
        assert!(validate_binary_frame(&valid).is_ok());
        type InvalidFrameCase = (fn(&mut BinaryFrame), FrameReject);
        let changes: [InvalidFrameCase; 9] = [
            (
                |f| f.metadata.protocol_version += 1,
                FrameReject::InvalidMetadata,
            ),
            (
                |f| f.metadata.workspace_id.clear(),
                FrameReject::InvalidMetadata,
            ),
            (
                |f| f.metadata.session_id = "x".repeat(257),
                FrameReject::InvalidMetadata,
            ),
            (|f| f.metadata.generation = 0, FrameReject::InvalidMetadata),
            (|f| f.metadata.sequence = 0, FrameReject::InvalidMetadata),
            (|f| f.metadata.width = 8192, FrameReject::InvalidDimensions),
            (|f| f.metadata.height = 0, FrameReject::InvalidDimensions),
            (|f| f.metadata.rotation = 90, FrameReject::InvalidRotation),
            (|f| f.payload[0] = 0, FrameReject::InvalidEncoding),
        ];
        for (change, expected) in changes {
            let mut frame = valid.clone();
            change(&mut frame);
            assert_eq!(validate_binary_frame(&frame), Err(expected));
        }
        let mut oversized = valid;
        oversized.payload = vec![0u8; MAX_JPEG_PAYLOAD_BYTES + 1];
        oversized.metadata.payload_length = oversized.payload.len() as u32;
        assert_eq!(
            validate_binary_frame(&oversized),
            Err(FrameReject::PayloadTooLarge)
        );
        let mut invalid_task = task();
        invalid_task.session_id.clear();
        let mut mailbox = BinaryFrameMailbox::new();
        mailbox.set_stream(&invalid_task);
        assert_eq!(mailbox.push(envelope(1)), Err(FrameReject::Stopped));
    }

    #[test]
    fn metrics_saturate_without_affecting_frame_delivery() {
        let mut mailbox = BinaryFrameMailbox::new();
        mailbox.set_stream(&task());
        mailbox.push(envelope(1)).unwrap();
        mailbox.metrics.received = u64::MAX;
        mailbox.metrics.dropped = u64::MAX;
        mailbox.metrics.rejected = u64::MAX;
        mailbox.metrics.bytes_received = u64::MAX;
        mailbox.push(envelope(2)).unwrap();
        assert_eq!(mailbox.push(envelope(1)), Err(FrameReject::OldSequence));
        assert_eq!(mailbox.metrics().received, u64::MAX);
        assert_eq!(mailbox.metrics().dropped, u64::MAX);
        assert_eq!(mailbox.metrics().rejected, u64::MAX);
        assert_eq!(mailbox.metrics().bytes_received, u64::MAX);
        assert_eq!(mailbox.take().unwrap().metadata.sequence, 2);
    }

    #[test]
    fn binary_packet_roundtrip_keeps_metadata_and_untouched_jpeg_separate() {
        let frame = envelope(1);
        let packet = frame.packet().unwrap();
        let metadata_length = u32::from_le_bytes(packet[..4].try_into().unwrap()) as usize;
        assert!(metadata_length <= MAX_BINARY_METADATA_BYTES);
        let metadata: BinaryFrameMetadata =
            serde_json::from_slice(&packet[4..4 + metadata_length]).unwrap();
        assert_eq!(metadata, frame.metadata);
        assert_eq!(metadata.protocol_version, BINARY_FRAME_PROTOCOL_VERSION);
        assert_eq!(packet[4 + metadata_length..], frame.payload);
        assert_eq!(
            metadata.payload_length as usize,
            packet.len() - 4 - metadata_length
        );
    }

    #[test]
    fn binary_packet_rejects_invalid_or_oversized_json_metadata() {
        let mut invalid = envelope(1);
        invalid.metadata.protocol_version += 1;
        assert!(invalid.packet().is_err());
        let mut oversized = envelope(1);
        // JSON escapes each control byte to six bytes, though the identities are bounded.
        oversized.metadata.workspace_id = "\u{1}".repeat(MAX_ID_BYTES);
        oversized.metadata.session_id = "\u{1}".repeat(MAX_ID_BYTES);
        oversized.metadata.build_id = "\u{1}".repeat(MAX_ID_BYTES);
        assert_eq!(oversized.packet().unwrap_err(), "FRAME_METADATA_LIMIT");
    }

    #[cfg(windows)]
    mod jpeg {
        use super::*;

        // Synthetic JPEGs exercise the parser only; they do not prove ArkUI rendering.
        fn packet(width: u32, height: u32) -> Vec<u8> {
            let pixels = image::RgbImage::from_pixel(width, height, image::Rgb([20, 40, 60]));
            let mut jpeg = Cursor::new(Vec::new());
            DynamicImage::ImageRgb8(pixels)
                .write_to(&mut jpeg, ImageFormat::Jpeg)
                .unwrap();
            let mut packet = vec![0u8; JPEG40_HEADER_BYTES];
            packet[..4].copy_from_slice(&[0x12, 0x34, 0x56, 0x78]);
            for offset in [4, 12] {
                packet[offset..offset + 4].copy_from_slice(&SURFACE_WIDTH.to_be_bytes());
                packet[offset + 4..offset + 8].copy_from_slice(&SURFACE_HEIGHT.to_be_bytes());
            }
            packet.extend_from_slice(jpeg.get_ref());
            packet
        }

        #[test]
        fn parser_preserves_real_jpeg_bytes_and_the_complete_lifecycle_identity() {
            let packet = packet(SURFACE_WIDTH, SURFACE_HEIGHT);
            let task = task();
            let frame = parse_jpeg40(&packet, &task, 7, 123).unwrap();
            assert_eq!(frame.payload, packet[JPEG40_HEADER_BYTES..]);
            assert_eq!(frame.metadata.workspace_id, task.workspace_id);
            assert_eq!(frame.metadata.session_id, task.session_id);
            assert_eq!(frame.metadata.build_id, task.build_id);
            assert_eq!(frame.metadata.generation, task.generation);
            assert_eq!(frame.metadata.sequence, 7);
            assert_eq!(frame.metadata.captured_at, 123);
            assert_eq!(frame.metadata.payload_length as usize, frame.payload.len());
            assert_eq!(frame.metadata.format, NativeFrameFormat::Jpeg);
        }

        #[test]
        fn malformed_headers_truncation_and_oversized_packets_fail_closed() {
            let valid = packet(SURFACE_WIDTH, SURFACE_HEIGHT);
            for length in 0..JPEG40_HEADER_BYTES + 4 {
                assert!(parse_jpeg40(&valid[..length], &task(), 1, 1).is_err());
            }
            for offset in [0, 4, 8, 12, 16, 20, 39] {
                let mut malformed = valid.clone();
                malformed[offset] ^= 1;
                assert!(parse_jpeg40(&malformed, &task(), 1, 1).is_err());
            }
            let oversized = vec![0u8; MAX_JPEG40_PACKET_BYTES + 1];
            assert_eq!(
                parse_jpeg40(&oversized, &task(), 1, 1).unwrap_err(),
                "FRAME_HEADER"
            );
            let mut truncated = valid;
            truncated.truncate(truncated.len() - 2);
            assert!(parse_jpeg40(&truncated, &task(), 1, 1).is_err());
        }

        #[test]
        fn invalid_jpeg_content_and_decoder_dimension_limits_fail_closed() {
            let valid = packet(SURFACE_WIDTH, SURFACE_HEIGHT);
            let mut invalid = valid[..JPEG40_HEADER_BYTES].to_vec();
            invalid.extend_from_slice(&[0xff, 0xd8, 0xff, 0xd9]);
            assert!(parse_jpeg40(&invalid, &task(), 1, 1)
                .unwrap_err()
                .starts_with("FRAME_DECODE:"));
            assert_eq!(
                parse_jpeg40(&packet(1, 1), &task(), 1, 1).unwrap_err(),
                "FRAME_DIMENSIONS_MISMATCH"
            );
            assert!(parse_jpeg40(&packet(361, 640), &task(), 1, 1).is_err());
            assert!(parse_jpeg40(&packet(360, 641), &task(), 1, 1).is_err());
            assert!(parse_jpeg40(&valid, &task(), 0, 1).is_err());
        }

        #[test]
        fn legacy_png_conversion_is_explicit_and_preserves_metadata() {
            let packet = packet(SURFACE_WIDTH, SURFACE_HEIGHT);
            let frame = parse_jpeg40(&packet, &task(), 7, 123).unwrap();
            let legacy = frame.legacy_png().unwrap();
            assert_eq!(legacy.session_id, frame.metadata.session_id);
            assert_eq!(legacy.build_id, frame.metadata.build_id);
            assert_eq!(legacy.generation, frame.metadata.generation);
            assert_eq!(legacy.sequence, frame.metadata.sequence);
            assert_eq!(legacy.captured_at, frame.metadata.captured_at);
            let png = STANDARD.decode(&legacy.png_base64).unwrap();
            assert!(png.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
            let image = image::load_from_memory_with_format(&png, ImageFormat::Png).unwrap();
            assert_eq!(
                (image.width(), image.height()),
                (SURFACE_WIDTH, SURFACE_HEIGHT)
            );
            assert_eq!(frame.payload, packet[JPEG40_HEADER_BYTES..]);
            let mut malformed = frame;
            malformed.payload = vec![0xff, 0xd8, 0xff, 0xd9];
            malformed.metadata.payload_length = 4;
            assert!(malformed.legacy_png().is_err());
        }
    }
}
