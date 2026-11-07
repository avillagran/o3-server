//! Complete pinned nightly4314 catalog. Public requests select an existing ID.
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Clone, Debug)]
pub struct Model {
    id: String,
    revision: String,
    filename: String,
    size: u64,
    sha256: String,
    value: Value,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidManifest;
impl std::fmt::Display for InvalidManifest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid speech model manifest")
    }
}
impl std::error::Error for InvalidManifest {}
impl Model {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub fn filename(&self) -> &str {
        &self.filename
    }
    pub fn size(&self) -> u64 {
        self.size
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Application-owned manifests only; never deserialize arbitrary client URLs.
    pub fn from_manifest(value: Value) -> Result<Self, InvalidManifest> {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(InvalidManifest)
        };
        let id = text("id")?;
        let revision = text("revision")?;
        let filename = text("filename")?;
        let sha256 = text("sha256")?;
        let size = value
            .get("size")
            .and_then(Value::as_u64)
            .filter(|size| *size > 0 && *size <= 9_007_199_254_740_991)
            .ok_or(InvalidManifest)?;
        let hex = |text: &str, len| {
            text.len() == len
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        };
        if id.split('/').count() != 2
            || id.split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || !part
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            })
            || !hex(&revision, 40)
            || !hex(&sha256, 64)
            || filename.is_empty()
            || filename == "."
            || filename == ".."
            || !filename
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        {
            return Err(InvalidManifest);
        }
        Ok(Self {
            id,
            revision,
            filename,
            size,
            sha256,
            value,
        })
    }
    pub fn as_value(&self) -> &Value {
        &self.value
    }
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.id, self.revision, self.filename
        )
    }
}
pub const DEFAULT_MODEL_ID: &str = "handy-computer/parakeet-unified-en-0.6b-gguf";
pub fn all() -> &'static [Model] {
    static CATALOG: OnceLock<Vec<Model>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let catalog: Value =
            serde_json::from_str(include_str!("../runtime/speech/catalog-4314.json"))
                .expect("Pinned speech catalog JSON");
        catalog["models"]
            .as_array()
            .expect("Pinned speech catalog models")
            .iter()
            .cloned()
            .map(|mut value| {
                value["accuracy"] = Value::from(0);
                value["speed"] = Value::from(0);
                Model::from_manifest(value).expect("Pinned speech catalog model")
            })
            .collect()
    })
}
pub fn get(id: &str) -> Option<&'static Model> {
    all().iter().find(|model| model.id == id)
}
pub fn selected(id: &str) -> &'static Model {
    get(id).unwrap_or_else(|| get(DEFAULT_MODEL_ID).expect("Pinned default speech model"))
}
