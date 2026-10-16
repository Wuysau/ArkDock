//! A real device capture has its own packet, never a fabricated Previewer header.
use crate::ipc::model::DeviceFrameMetadata;
use std::io::Read;
use std::path::Path;

pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
pub struct DeviceFrame {
    pub metadata: DeviceFrameMetadata,
    pub png: Vec<u8>,
}
impl DeviceFrame {
    pub fn from_file(path: &Path, mut metadata: DeviceFrameMetadata) -> Result<Self, String> {
        let mut png = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| "DEVICE_CAPTURE_FILE_MISSING")?
            .take((MAX_PNG_BYTES + 1) as u64)
            .read_to_end(&mut png)
            .map_err(|_| "DEVICE_CAPTURE_READ_FAILED")?;
        let (width, height) = validate_png(&png)?;
        metadata.width = width;
        metadata.height = height;
        metadata.payload_length = png.len() as u32;
        Ok(Self { metadata, png })
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let metadata = serde_json::to_vec(&self.metadata).map_err(|_| "DEVICE_FRAME_METADATA")?;
        if metadata.len() > 4096 || self.png.len() > MAX_PNG_BYTES {
            return Err("DEVICE_FRAME_LIMIT".into());
        }
        let mut bytes = Vec::with_capacity(16 + metadata.len() + self.png.len());
        bytes.extend_from_slice(b"ADDF");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.png.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&metadata);
        bytes.extend_from_slice(&self.png);
        Ok(bytes)
    }
}
fn validate_png(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() < 24 || bytes.len() > MAX_PNG_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("DEVICE_CAPTURE_PNG_INVALID".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || u64::from(width) * u64::from(height) > 8_388_608
    {
        return Err("DEVICE_CAPTURE_DIMENSIONS_INVALID".into());
    }
    #[cfg(windows)]
    {
        let mut reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(4096);
        limits.max_alloc = Some(64 * 1024 * 1024);
        reader.limits(limits);
        let decoded = reader.decode().map_err(|_| "DEVICE_CAPTURE_PNG_INVALID")?;
        if decoded.width() != width || decoded.height() != height {
            return Err("DEVICE_CAPTURE_DIMENSIONS_INVALID".into());
        }
        Ok((width, height))
    }
    #[cfg(not(windows))]
    {
        Err("DEVICE_RUNTIME_UNSUPPORTED_PLATFORM".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_frames_and_oversized_dimensions_before_decoding() {
        assert!(validate_png(b"success").is_err());
        let mut bytes = vec![0; 24];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes[16..20].copy_from_slice(&99999u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&99999u32.to_be_bytes());
        assert_eq!(
            validate_png(&bytes).unwrap_err(),
            "DEVICE_CAPTURE_DIMENSIONS_INVALID"
        );
    }
    #[test]
    fn device_packet_has_independent_identity_and_bounded_lengths() {
        let frame = DeviceFrame {
            metadata: DeviceFrameMetadata {
                run_id: "run".into(),
                target_id: "device".into(),
                bundle_name: "app.test".into(),
                sequence: 1,
                width: 1,
                height: 1,
                capture_started_at: 10,
                received_at: 12,
                payload_length: 3,
                foreground_verified: false,
            },
            png: vec![1, 2, 3],
        };
        let packet = frame.encode().unwrap();
        assert_eq!(&packet[..4], b"ADDF");
        let length = u32::from_le_bytes(packet[8..12].try_into().unwrap()) as usize;
        let metadata: DeviceFrameMetadata =
            serde_json::from_slice(&packet[16..16 + length]).unwrap();
        assert_eq!(metadata.target_id, "device");
        assert_eq!(&packet[16 + length..], &[1, 2, 3]);
    }
}
