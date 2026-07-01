use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::Path;

pub type MetadataMap = BTreeMap<String, MetadataValue>;
pub type LensResult<T> = Result<T, LensError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub analyzer: AnalyzerContract,
    pub viewer: ViewerContract,
}

impl LensManifest {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        analyzer: AnalyzerContract,
        viewer: ViewerContract,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            version: version.into(),
            analyzer,
            viewer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerContract {
    pub supported_media_types: Vec<String>,
    pub file_extensions: Vec<String>,
    pub capabilities: Vec<LensCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LensCapability {
    View,
    Diff,
    Reconcile,
    Metadata,
    Preview,
    References,
    ProofRecipes,
    Custom(String),
}

#[derive(Debug, Clone, Copy)]
pub struct AnalysisInput<'a> {
    pub artifact_id: &'a str,
    pub path: Option<&'a Path>,
    pub media_type: Option<&'a str>,
    pub bytes: &'a [u8],
    pub previous_bytes: Option<&'a [u8]>,
}

impl<'a> AnalysisInput<'a> {
    pub fn new(artifact_id: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            artifact_id,
            path: None,
            media_type: None,
            bytes,
            previous_bytes: None,
        }
    }

    pub fn with_path(mut self, path: &'a Path) -> Self {
        self.path = Some(path);
        self
    }

    pub fn with_media_type(mut self, media_type: &'a str) -> Self {
        self.media_type = Some(media_type);
        self
    }

    pub fn with_previous_bytes(mut self, previous_bytes: &'a [u8]) -> Self {
        self.previous_bytes = Some(previous_bytes);
        self
    }
}

pub trait Analyzer {
    fn manifest(&self) -> LensManifest;
    fn analyze(&self, input: AnalysisInput<'_>) -> LensResult<AnalysisOutput>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisOutput {
    pub metadata: ArtifactMetadata,
    pub preview: Option<PreviewModel>,
    pub diff: Option<DiffModel>,
    pub reconcile: ReconcileModel,
    pub references: Vec<ExtractedReference>,
    pub proof_recipes: Vec<ProofRecipe>,
}

impl AnalysisOutput {
    pub fn new(metadata: ArtifactMetadata) -> Self {
        Self {
            metadata,
            preview: None,
            diff: None,
            reconcile: ReconcileModel::unsupported(
                "Reconciliation is not implemented for this lens",
            ),
            references: Vec::new(),
            proof_recipes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactMetadata {
    pub artifact_id: String,
    pub lens_id: String,
    pub kind: ArtifactKind,
    pub media_type: String,
    pub byte_len: u64,
    pub content_hash: String,
    pub fields: MetadataMap,
}

impl ArtifactMetadata {
    pub fn new(
        artifact_id: impl Into<String>,
        lens_id: impl Into<String>,
        kind: ArtifactKind,
        media_type: impl Into<String>,
        bytes: &[u8],
    ) -> Self {
        Self {
            artifact_id: artifact_id.into(),
            lens_id: lens_id.into(),
            kind,
            media_type: media_type.into(),
            byte_len: bytes.len() as u64,
            content_hash: content_hash(bytes),
            fields: MetadataMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactKind {
    Raw,
    Text,
    Code,
    Image,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    String(String),
    Integer(i64),
    Unsigned(u64),
    Float(f64),
    Bool(bool),
    StringList(Vec<String>),
}

impl From<&str> for MetadataValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<String> for MetadataValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<u64> for MetadataValue {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<u32> for MetadataValue {
    fn from(value: u32) -> Self {
        Self::Unsigned(u64::from(value))
    }
}

impl From<usize> for MetadataValue {
    fn from(value: usize) -> Self {
        Self::Unsigned(value as u64)
    }
}

impl From<i64> for MetadataValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<bool> for MetadataValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewModel {
    pub kind: PreviewKind,
    pub title: String,
    pub body: Option<String>,
    pub media_type: String,
    pub language: Option<String>,
    pub dimensions: Option<Dimensions>,
    pub fields: MetadataMap,
}

impl PreviewModel {
    pub fn new(kind: PreviewKind, title: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            body: None,
            media_type: media_type.into(),
            language: None,
            dimensions: None,
            fields: MetadataMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewKind {
    Raw,
    Text,
    Code,
    Image,
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffModel {
    pub kind: DiffKind,
    pub summary: String,
    pub hunks: Vec<DiffHunk>,
    pub fields: MetadataMap,
}

impl DiffModel {
    pub fn new(kind: DiffKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
            hunks: Vec::new(),
            fields: MetadataMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    TextLines,
    Binary,
    ImageMetadata,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub op: DiffOp,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffOp {
    Equal,
    Insert,
    Delete,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReconcileModel {
    pub status: ReconcileStatus,
    pub summary: String,
    pub fields: MetadataMap,
}

impl ReconcileModel {
    pub fn new(status: ReconcileStatus, summary: impl Into<String>) -> Self {
        Self {
            status,
            summary: summary.into(),
            fields: MetadataMap::new(),
        }
    }

    pub fn unsupported(summary: impl Into<String>) -> Self {
        Self::new(ReconcileStatus::Unsupported, summary)
    }

    pub fn needs_manual_resolution(summary: impl Into<String>) -> Self {
        Self::new(ReconcileStatus::NeedsManualResolution, summary)
    }

    pub fn auto_resolvable(summary: impl Into<String>) -> Self {
        Self::new(ReconcileStatus::AutoResolvable, summary)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileStatus {
    Unsupported,
    NeedsManualResolution,
    AutoResolvable,
}

impl ReconcileStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::NeedsManualResolution => "needs_manual_resolution",
            Self::AutoResolvable => "auto_resolvable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedReference {
    pub target: String,
    pub kind: ReferenceKind,
    pub span: TextSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceKind {
    RelativePath,
    UrlFunction,
    ExternalUrl,
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSpan {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerContract {
    pub viewer_id: String,
    pub schema_version: String,
    pub component: String,
    pub preview_kinds: Vec<PreviewKind>,
    pub diff_kinds: Vec<DiffKind>,
    pub reconcile_statuses: Vec<ReconcileStatus>,
    pub inspector_fields: Vec<InspectorField>,
}

impl ViewerContract {
    pub fn new(
        viewer_id: impl Into<String>,
        component: impl Into<String>,
        preview_kinds: Vec<PreviewKind>,
        diff_kinds: Vec<DiffKind>,
    ) -> Self {
        Self {
            viewer_id: viewer_id.into(),
            schema_version: "layrs.viewer.v1".to_string(),
            component: component.into(),
            preview_kinds,
            diff_kinds,
            reconcile_statuses: vec![ReconcileStatus::Unsupported],
            inspector_fields: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorField {
    pub key: String,
    pub label: String,
    pub value_type: InspectorValueType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectorValueType {
    String,
    Number,
    Boolean,
    StringList,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofRecipe {
    pub id: String,
    pub title: String,
    pub description: String,
    pub checks: Vec<ProofCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofCheck {
    pub subject: String,
    pub expectation: String,
    pub observed: Option<String>,
    pub status: ProofStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofStatus {
    Pass,
    Warn,
    NotEvaluated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensError {
    pub code: String,
    pub message: String,
}

impl LensError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for LensError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl Error for LensError {}

pub fn content_hash(bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    format!("fnv1a64:{hash:016x}")
}

pub fn infer_media_type_from_path(path: Option<&Path>) -> Option<&'static str> {
    let extension = path
        .and_then(Path::extension)
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();

    match extension.as_str() {
        "txt" | "md" | "markdown" | "rst" | "log" => Some("text/plain"),
        "rs" => Some("text/rust"),
        "go" => Some("text/x-go"),
        "py" => Some("text/x-python"),
        "ts" | "tsx" => Some("text/typescript"),
        "js" | "jsx" => Some("text/javascript"),
        "json" => Some("application/json"),
        "css" => Some("text/css"),
        "html" | "htm" => Some("text/html"),
        "toml" => Some("application/toml"),
        "yaml" | "yml" => Some("application/yaml"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_output_declares_reconcile_unsupported() {
        let metadata = ArtifactMetadata::new(
            "artifact",
            "layrs.test",
            ArtifactKind::Raw,
            "application/octet-stream",
            b"bytes",
        );

        let output = AnalysisOutput::new(metadata);

        assert_eq!(output.reconcile.status, ReconcileStatus::Unsupported);
    }

    #[test]
    fn viewer_contract_exposes_reconcile_statuses() {
        let viewer = ViewerContract::new(
            "viewer",
            "Component",
            vec![PreviewKind::Text],
            vec![DiffKind::TextLines],
        );

        assert_eq!(
            viewer.reconcile_statuses,
            vec![ReconcileStatus::Unsupported]
        );
    }

    #[test]
    fn reconcile_statuses_have_stable_wire_labels() {
        assert_eq!(ReconcileStatus::Unsupported.as_str(), "unsupported");
        assert_eq!(
            ReconcileStatus::NeedsManualResolution.as_str(),
            "needs_manual_resolution"
        );
        assert_eq!(ReconcileStatus::AutoResolvable.as_str(), "auto_resolvable");
    }
}

pub fn span_at(text: &str, start: usize, end: usize) -> TextSpan {
    let mut line = 1usize;
    let mut column = 1usize;

    for (offset, ch) in text.char_indices() {
        if offset >= start {
            break;
        }

        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    TextSpan {
        start,
        end,
        line,
        column,
    }
}
