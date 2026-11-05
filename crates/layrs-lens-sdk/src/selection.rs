use std::error::Error;
use std::fmt;

use crate::LensManifest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LensMatchReason {
    Explicit,
    MediaType,
    Extension,
    Fallback,
}

impl LensMatchReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::MediaType => "mediaType",
            Self::Extension => "extension",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LensSelectionRequest<'a> {
    pub lens_id: Option<&'a str>,
    pub path: Option<&'a str>,
    pub media_type: Option<&'a str>,
    pub fallback_lens_id: Option<&'a str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LensSelectionError {
    code: &'static str,
    message: String,
}

impl LensSelectionError {
    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LensSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for LensSelectionError {}

pub fn select_lens_manifest<'a>(
    manifests: &'a [LensManifest],
    request: LensSelectionRequest<'_>,
) -> Result<Option<(&'a LensManifest, LensMatchReason)>, LensSelectionError> {
    if let Some(lens_id) = request.lens_id {
        return unique_lens_match(manifests, "lens id", lens_id, |manifest| {
            manifest.id == lens_id
        })
        .map(|manifest| manifest.map(|manifest| (manifest, LensMatchReason::Explicit)));
    }

    if let Some(media_type) = normalize_media_type(request.media_type) {
        if let Some(manifest) =
            unique_lens_match(manifests, "media type", &media_type, |manifest| {
                manifest
                    .analyzer
                    .supported_media_types
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(&media_type))
            })?
        {
            return Ok(Some((manifest, LensMatchReason::MediaType)));
        }
    }

    if let Some(extension) = extension_from_path(request.path) {
        if let Some(manifest) = unique_lens_match(manifests, "extension", &extension, |manifest| {
            manifest
                .analyzer
                .file_extensions
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(&extension))
        })? {
            return Ok(Some((manifest, LensMatchReason::Extension)));
        }
    }

    if let Some(fallback_lens_id) = request.fallback_lens_id {
        return unique_lens_match(
            manifests,
            "fallback lens id",
            fallback_lens_id,
            |manifest| manifest.id == fallback_lens_id,
        )
        .map(|manifest| manifest.map(|manifest| (manifest, LensMatchReason::Fallback)));
    }
    Ok(None)
}

fn unique_lens_match<'a>(
    manifests: &'a [LensManifest],
    criterion: &str,
    value: &str,
    predicate: impl Fn(&LensManifest) -> bool,
) -> Result<Option<&'a LensManifest>, LensSelectionError> {
    let matches = manifests
        .iter()
        .filter(|manifest| predicate(manifest))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [manifest] => Ok(Some(*manifest)),
        _ => {
            let mut ids = matches
                .iter()
                .map(|manifest| manifest.id.as_str())
                .collect::<Vec<_>>();
            ids.sort_unstable();
            Err(LensSelectionError {
                code: "ambiguous_lens_match",
                message: format!(
                    "{criterion} `{value}` matches multiple Lenses: {}",
                    ids.join(", ")
                ),
            })
        }
    }
}

fn normalize_media_type(media_type: Option<&str>) -> Option<String> {
    media_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn extension_from_path(path: Option<&str>) -> Option<String> {
    let name = path?.rsplit(['/', '\\']).next()?;
    let (_, extension) = name.rsplit_once('.')?;
    (!extension.is_empty()).then(|| extension.to_ascii_lowercase())
}
