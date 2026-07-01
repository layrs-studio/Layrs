use layrs_lens_sdk::{
    AnalysisInput, AnalysisOutput, Analyzer, AnalyzerContract, ArtifactKind, ArtifactMetadata,
    DiffKind, DiffModel, Dimensions, InspectorField, InspectorValueType, LensCapability,
    LensManifest, LensResult, MetadataValue, PreviewKind, PreviewModel, ProofCheck, ProofRecipe,
    ProofStatus, ReconcileModel, ReconcileStatus, ViewerContract, infer_media_type_from_path,
};

pub const IMAGE_LENS_ID: &str = "layrs.image";
pub const MAX_PREVIEW_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_DECODED_PIXELS: u64 = 20_000_000;

#[derive(Debug, Default, Clone, Copy)]
pub struct ImageLens;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    pub format: ImageFormat,
    pub dimensions: Option<Dimensions>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    WebP,
    Unknown,
}

impl ImageFormat {
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::WebP => "image/webp",
            Self::Unknown => "application/octet-stream",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::WebP => "webp",
            Self::Unknown => "unknown",
        }
    }
}

impl Analyzer for ImageLens {
    fn manifest(&self) -> LensManifest {
        manifest()
    }

    fn analyze(&self, input: AnalysisInput<'_>) -> LensResult<AnalysisOutput> {
        Ok(analyze(input))
    }
}

pub fn manifest() -> LensManifest {
    let mut viewer = ViewerContract::new(
        "layrs.viewer.image",
        "ImageArtifactViewer",
        vec![PreviewKind::Image],
        vec![DiffKind::ImageMetadata],
    );
    viewer.reconcile_statuses = reconcile_statuses_v1();
    viewer.inspector_fields = vec![
        InspectorField {
            key: "format".to_string(),
            label: "Format".to_string(),
            value_type: InspectorValueType::String,
        },
        InspectorField {
            key: "width".to_string(),
            label: "Width".to_string(),
            value_type: InspectorValueType::Number,
        },
        InspectorField {
            key: "height".to_string(),
            label: "Height".to_string(),
            value_type: InspectorValueType::Number,
        },
        InspectorField {
            key: "pixel_count".to_string(),
            label: "Pixels".to_string(),
            value_type: InspectorValueType::Number,
        },
    ];

    LensManifest::new(
        IMAGE_LENS_ID,
        "Image",
        "0.0.0",
        AnalyzerContract {
            supported_media_types: vec![
                "image/png".to_string(),
                "image/jpeg".to_string(),
                "image/webp".to_string(),
            ],
            file_extensions: vec![
                "png".to_string(),
                "jpg".to_string(),
                "jpeg".to_string(),
                "webp".to_string(),
            ],
            capabilities: vec![
                LensCapability::View,
                LensCapability::Diff,
                LensCapability::Reconcile,
                LensCapability::Metadata,
                LensCapability::Preview,
                LensCapability::ProofRecipes,
            ],
        },
        viewer,
    )
}

fn reconcile_statuses_v1() -> Vec<ReconcileStatus> {
    vec![
        ReconcileStatus::Unsupported,
        ReconcileStatus::NeedsManualResolution,
        ReconcileStatus::AutoResolvable,
    ]
}

pub fn analyze(input: AnalysisInput<'_>) -> AnalysisOutput {
    let info = detect_image(input.bytes);
    let media_type = input
        .media_type
        .or_else(|| match info.format {
            ImageFormat::Unknown => infer_media_type_from_path(input.path),
            format => Some(format.media_type()),
        })
        .unwrap_or_else(|| info.format.media_type());

    let mut metadata = ArtifactMetadata::new(
        input.artifact_id,
        IMAGE_LENS_ID,
        ArtifactKind::Image,
        media_type,
        input.bytes,
    );
    metadata.fields.insert(
        "format".to_string(),
        MetadataValue::String(info.format.label().to_string()),
    );

    if let Some(dimensions) = info.dimensions {
        metadata.fields.insert(
            "width".to_string(),
            MetadataValue::Unsigned(u64::from(dimensions.width)),
        );
        metadata.fields.insert(
            "height".to_string(),
            MetadataValue::Unsigned(u64::from(dimensions.height)),
        );
        metadata.fields.insert(
            "pixel_count".to_string(),
            MetadataValue::Unsigned(u64::from(dimensions.width) * u64::from(dimensions.height)),
        );
    }

    let mut preview = PreviewModel::new(PreviewKind::Image, "Image artifact", media_type);
    preview.dimensions = info.dimensions;
    preview.fields.insert(
        "format".to_string(),
        MetadataValue::String(info.format.label().to_string()),
    );

    let mut output = AnalysisOutput::new(metadata);
    output.preview = Some(preview);
    output.diff = input
        .previous_bytes
        .map(|previous| diff_image_metadata(detect_image(previous), info));
    output.reconcile = reconcile_for_image_diff(output.diff.as_ref());
    output.proof_recipes.push(image_budget_recipe(
        input.bytes.len() as u64,
        info.dimensions,
    ));
    output
}

pub fn diff_image_metadata(previous: ImageInfo, current: ImageInfo) -> DiffModel {
    let changed = previous != current;
    let mut diff = DiffModel::new(
        DiffKind::ImageMetadata,
        if changed {
            "Image metadata changed"
        } else {
            "No image metadata changes"
        },
    );
    diff.fields.insert(
        "previous_format".to_string(),
        MetadataValue::String(previous.format.label().to_string()),
    );
    diff.fields.insert(
        "current_format".to_string(),
        MetadataValue::String(current.format.label().to_string()),
    );
    if let Some(dimensions) = previous.dimensions {
        diff.fields.insert(
            "previous_width".to_string(),
            MetadataValue::Unsigned(u64::from(dimensions.width)),
        );
        diff.fields.insert(
            "previous_height".to_string(),
            MetadataValue::Unsigned(u64::from(dimensions.height)),
        );
    }
    if let Some(dimensions) = current.dimensions {
        diff.fields.insert(
            "current_width".to_string(),
            MetadataValue::Unsigned(u64::from(dimensions.width)),
        );
        diff.fields.insert(
            "current_height".to_string(),
            MetadataValue::Unsigned(u64::from(dimensions.height)),
        );
    }
    diff.fields
        .insert("changed".to_string(), MetadataValue::Bool(changed));
    diff
}

pub fn reconcile_for_image_diff(diff: Option<&DiffModel>) -> ReconcileModel {
    match diff.and_then(|diff| diff.fields.get("changed")) {
        Some(MetadataValue::Bool(false)) => {
            ReconcileModel::auto_resolvable("No image metadata changes to reconcile")
        }
        Some(MetadataValue::Bool(true)) => {
            ReconcileModel::unsupported("Image reconciliation is not implemented in V1")
        }
        _ => ReconcileModel::unsupported("Reconciliation requires a previous image artifact"),
    }
}

pub fn detect_image(bytes: &[u8]) -> ImageInfo {
    if let Some((width, height)) = parse_png_dimensions(bytes) {
        return ImageInfo {
            format: ImageFormat::Png,
            dimensions: Some(Dimensions { width, height }),
        };
    }

    if is_png(bytes) {
        return ImageInfo {
            format: ImageFormat::Png,
            dimensions: None,
        };
    }

    if is_jpeg(bytes) {
        return ImageInfo {
            format: ImageFormat::Jpeg,
            dimensions: parse_jpeg_dimensions(bytes)
                .map(|(width, height)| Dimensions { width, height }),
        };
    }

    if is_webp(bytes) {
        return ImageInfo {
            format: ImageFormat::WebP,
            dimensions: parse_webp_dimensions(bytes)
                .map(|(width, height)| Dimensions { width, height }),
        };
    }

    ImageInfo {
        format: ImageFormat::Unknown,
        dimensions: None,
    }
}

pub fn parse_png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || !is_png(bytes) || &bytes[12..16] != b"IHDR" {
        return None;
    }

    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    if width == 0 || height == 0 {
        return None;
    }

    Some((width, height))
}

pub fn is_png(bytes: &[u8]) -> bool {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    bytes.len() >= PNG_SIGNATURE.len() && &bytes[..PNG_SIGNATURE.len()] == PNG_SIGNATURE
}

pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff
}

pub fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

fn parse_jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if !is_jpeg(bytes) {
        return None;
    }

    let mut cursor = 2usize;
    while cursor + 4 < bytes.len() {
        while cursor < bytes.len() && bytes[cursor] == 0xff {
            cursor += 1;
        }

        if cursor >= bytes.len() {
            return None;
        }

        let marker = bytes[cursor];
        cursor += 1;

        if marker == 0xd9 || marker == 0xda {
            return None;
        }

        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }

        if cursor + 2 > bytes.len() {
            return None;
        }

        let segment_len = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
        if segment_len < 2 || cursor + segment_len > bytes.len() {
            return None;
        }

        if is_jpeg_sof_marker(marker) && segment_len >= 7 {
            let data = cursor + 2;
            let height = u16::from_be_bytes([bytes[data + 1], bytes[data + 2]]) as u32;
            let width = u16::from_be_bytes([bytes[data + 3], bytes[data + 4]]) as u32;
            if width > 0 && height > 0 {
                return Some((width, height));
            }
            return None;
        }

        cursor += segment_len;
    }

    None
}

fn is_jpeg_sof_marker(marker: u8) -> bool {
    matches!(
        marker,
        0xc0 | 0xc1 | 0xc2 | 0xc3 | 0xc5 | 0xc6 | 0xc7 | 0xc9 | 0xca | 0xcb | 0xcd | 0xce | 0xcf
    )
}

fn parse_webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 30 || !is_webp(bytes) || &bytes[12..16] != b"VP8X" {
        return None;
    }

    let width = read_webp_24bit(&bytes[24..27]) + 1;
    let height = read_webp_24bit(&bytes[27..30]) + 1;
    if width == 0 || height == 0 {
        return None;
    }

    Some((width, height))
}

fn read_webp_24bit(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

fn image_budget_recipe(byte_len: u64, dimensions: Option<Dimensions>) -> ProofRecipe {
    let byte_status = if byte_len <= MAX_PREVIEW_BYTES {
        ProofStatus::Pass
    } else {
        ProofStatus::Warn
    };
    let pixel_count =
        dimensions.map(|dimensions| u64::from(dimensions.width) * u64::from(dimensions.height));
    let pixel_status = match pixel_count {
        Some(pixels) if pixels <= MAX_DECODED_PIXELS => ProofStatus::Pass,
        Some(_) => ProofStatus::Warn,
        None => ProofStatus::NotEvaluated,
    };

    ProofRecipe {
        id: "layrs.image.budget.v1".to_string(),
        title: "Image preview budget".to_string(),
        description: "Checks byte and decoded-pixel budgets before Studio preview work."
            .to_string(),
        checks: vec![
            ProofCheck {
                subject: "encoded_bytes".to_string(),
                expectation: format!("<= {MAX_PREVIEW_BYTES}"),
                observed: Some(byte_len.to_string()),
                status: byte_status,
            },
            ProofCheck {
                subject: "decoded_pixels".to_string(),
                expectation: format!("<= {MAX_DECODED_PIXELS}"),
                observed: pixel_count.map(|pixels| pixels.to_string()),
                status: pixel_status,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_png_dimensions_from_ihdr() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&800u32.to_be_bytes());
        bytes.extend_from_slice(&600u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&0u32.to_be_bytes());

        assert_eq!(parse_png_dimensions(&bytes), Some((800, 600)));
        let info = detect_image(&bytes);
        assert_eq!(info.format, ImageFormat::Png);
        assert_eq!(
            info.dimensions,
            Some(Dimensions {
                width: 800,
                height: 600
            })
        );
    }

    #[test]
    fn rejects_zero_png_dimensions_but_keeps_png_format() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&600u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&0u32.to_be_bytes());

        assert_eq!(parse_png_dimensions(&bytes), None);
        assert_eq!(detect_image(&bytes).format, ImageFormat::Png);
    }

    #[test]
    fn manifest_declares_core_lens_capabilities() {
        let manifest = manifest();

        assert!(
            manifest
                .analyzer
                .capabilities
                .contains(&LensCapability::View)
        );
        assert!(
            manifest
                .analyzer
                .capabilities
                .contains(&LensCapability::Diff)
        );
        assert!(
            manifest
                .analyzer
                .capabilities
                .contains(&LensCapability::Reconcile)
        );
        assert_eq!(manifest.viewer.diff_kinds, vec![DiffKind::ImageMetadata]);
    }
}
