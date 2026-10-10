//! A room-classifier package on disk (the C# `ModelPackage`): `manifest.json` lists every file
//! with its SHA-256, and the encoder, tokenizer, head, preprocessing spec and taxonomy must all
//! be there and match before anything is used.
//!
//! [`ModelPackage::peek`] only reads the manifest's version (cheap, for the status line);
//! [`ModelPackage::load`] verifies every file (it reads the 90 MB encoder) and is meant for a
//! worker thread.

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The taxonomy this client knows how to draw.
pub const SUPPORTED_TAXONOMY: &str = "1.0.0";
const REQUIRED: [&str; 5] = [
    "encoder.onnx",
    "tokenizer.json",
    "head.json",
    "preprocessing_spec.json",
    "taxonomy.json",
];

/// Why a package cannot be used. The text is for the status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageError(pub String);

impl std::fmt::Display for PackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PackageError {}

fn bad(message: impl Into<String>) -> PackageError {
    PackageError(message.into())
}

/// A verified package: its version, threshold, word-piece limit, classes and logistic head.
#[derive(Clone, Debug)]
pub struct ModelPackage {
    pub directory: PathBuf,
    pub version: String,
    pub taxonomy_version: String,
    /// The package's default confidence threshold (0.8 in 0.1.1).
    pub threshold: f64,
    /// Word pieces per text, `[CLS]` and `[SEP]` included (256).
    pub max_word_pieces: usize,
    pub classes: Vec<String>,
    /// One row per class, `width` values each, row after row.
    pub coefficients: Vec<f32>,
    pub width: usize,
    pub intercepts: Vec<f32>,
}

/// `major.minor.patch`, digits only (also keeps a version safe as a folder name).
pub fn valid_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

fn read_json(path: &Path) -> Result<Value, PackageError> {
    let text = std::fs::read_to_string(path).map_err(|e| bad(format!("{}: {e}", file_name(path))))?;
    serde_json::from_str(&text).map_err(|_| bad("Malformed package metadata."))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(super::preprocess::hex(&hasher.finalize()))
}

impl ModelPackage {
    /// The version a package folder's manifest names, without verifying anything.
    pub fn peek(directory: &Path) -> Option<String> {
        let manifest = read_json(&directory.join("manifest.json")).ok()?;
        let version = manifest.get("version")?.as_str()?;
        valid_version(version).then(|| version.to_string())
    }

    /// Load and verify a package folder: every manifest entry is a plain file name whose
    /// SHA-256 matches, the required files are listed, the taxonomy is 1.0.0 and the head is
    /// consistent.
    pub fn load(directory: &Path) -> Result<ModelPackage, PackageError> {
        let manifest_path = directory.join("manifest.json");
        if !manifest_path.is_file() {
            return Err(bad("Missing manifest.json."));
        }
        let manifest = read_json(&manifest_path)?;
        let version = manifest
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("Missing package version."))?;
        if !valid_version(version) {
            return Err(bad("Invalid package version."));
        }
        let files = manifest
            .get("files")
            .and_then(Value::as_object)
            .ok_or_else(|| bad("Malformed package metadata."))?;
        for required in REQUIRED {
            if !files.contains_key(required) {
                return Err(bad(format!("Manifest does not list {required}.")));
            }
        }
        for (name, expected) in files {
            let plain = Path::new(name).file_name().is_some_and(|n| n == name.as_str());
            if name.is_empty() || name.starts_with('.') || !plain || name.contains(['/', '\\']) {
                return Err(bad("Unsafe manifest entry."));
            }
            let path = directory.join(name);
            if !path.is_file() {
                return Err(bad(format!("Missing {name}.")));
            }
            let actual = sha256_file(&path).map_err(|e| bad(format!("{name}: {e}")))?;
            if !expected.as_str().is_some_and(|e| e.eq_ignore_ascii_case(&actual)) {
                return Err(bad(format!("{name} failed hash verification.")));
            }
        }
        let spec = read_json(&directory.join("preprocessing_spec.json"))?;
        let taxonomy = read_json(&directory.join("taxonomy.json"))?;
        let taxonomy_version = taxonomy.get("version").and_then(Value::as_str).unwrap_or_default();
        if taxonomy_version != SUPPORTED_TAXONOMY {
            return Err(bad(format!("Unsupported taxonomy {taxonomy_version}.")));
        }
        let head = read_json(&directory.join("head.json"))?;
        let malformed = || bad("Malformed package metadata.");
        let classes: Vec<String> = head
            .get("classes")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|c| c.as_str().unwrap_or_default().to_string())
            .collect();
        let rows: Vec<Vec<f32>> = head
            .get("coef")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|row| {
                row.as_array()
                    .map(|r| r.iter().map(|v| v.as_f64().unwrap_or(f64::NAN) as f32).collect())
                    .unwrap_or_default()
            })
            .collect();
        let intercepts: Vec<f32> = head
            .get("intercept")
            .and_then(Value::as_array)
            .ok_or_else(malformed)?
            .iter()
            .map(|v| v.as_f64().unwrap_or(f64::NAN) as f32)
            .collect();
        let width = rows.first().map_or(0, Vec::len);
        if classes.is_empty()
            || rows.len() != classes.len()
            || intercepts.len() != classes.len()
            || width == 0
            || rows.iter().any(|r| r.len() != width)
            || rows.iter().flatten().chain(&intercepts).any(|v| !v.is_finite())
        {
            return Err(bad("Inconsistent classifier head."));
        }
        let threshold = spec.get("threshold").and_then(Value::as_f64).unwrap_or(0.8);
        let max_word_pieces = spec
            .get("max_word_pieces")
            .and_then(Value::as_u64)
            .map_or(256, |v| v as usize);
        if !(0.0..=1.0).contains(&threshold) || !(3..=4096).contains(&max_word_pieces) {
            return Err(malformed());
        }
        Ok(ModelPackage {
            directory: directory.to_path_buf(),
            version: version.to_string(),
            taxonomy_version: taxonomy_version.to_string(),
            threshold,
            max_word_pieces,
            classes,
            coefficients: rows.into_iter().flatten().collect(),
            width,
            intercepts,
        })
    }

    pub fn encoder_path(&self) -> PathBuf {
        self.directory.join("encoder.onnx")
    }

    /// The word-piece vocabulary (`tokenizer.json` `model.vocab`), indexed by id. Ids must run
    /// from 0 without gaps.
    pub fn vocabulary(&self) -> Result<Vec<String>, PackageError> {
        let tokenizer = read_json(&self.directory.join("tokenizer.json"))?;
        vocabulary_of(&tokenizer)
    }
}

/// The vocabulary of a `tokenizer.json` value, indexed by id.
pub fn vocabulary_of(tokenizer: &Value) -> Result<Vec<String>, PackageError> {
    let vocab = tokenizer
        .get("model")
        .and_then(|m| m.get("vocab"))
        .and_then(Value::as_object)
        .ok_or_else(|| bad("Malformed package metadata."))?;
    let mut pairs: Vec<(u64, &String)> = vocab
        .iter()
        .map(|(token, id)| (id.as_u64().unwrap_or(u64::MAX), token))
        .collect();
    pairs.sort_by_key(|p| p.0);
    if pairs.iter().enumerate().any(|(i, p)| p.0 != i as u64) {
        return Err(bad("Vocabulary ids are not contiguous."));
    }
    Ok(pairs.into_iter().map(|(_, t)| t.clone()).collect())
}
