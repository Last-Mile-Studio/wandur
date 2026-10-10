//! The installed classifier package and its lazily created classifier (the C#
//! `RoomClassificationService` and `ModelPackageInstaller`, without the download).
//!
//! Packages live in `<data dir>/models/room-classifier/<version>/`. Creating the service only
//! reads manifest versions; verifying the package (hashing every file) and loading the encoder
//! happen the first time a worker asks for the classifier. Install from file takes a package
//! `.zip` or an extracted package folder, verifies it in a staging folder and moves it into
//! place.
//!
//! Ruling: no Download button. The C# client downloads the package from GitHub; this client
//! makes no request to an outside host in this run, so a package is installed from a local copy
//! (Install from file, or `WANDUR_ROOM_MODEL_DIR` naming a package folder for development).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::RoomClassifier;
use super::package::{ModelPackage, PackageError};

/// Largest package accepted (the C# 200 MB).
pub const MAX_PACKAGE_BYTES: u64 = 200 * 1024 * 1024;
const MAX_ENTRIES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassificationState {
    /// This build has no classifier (the `classifier` feature is off).
    Unavailable,
    NotInstalled,
    /// Install from file in progress.
    Installing,
    Ready,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassificationStatus {
    pub state: ClassificationState,
    pub version: Option<String>,
    pub message: Option<String>,
}

impl ClassificationStatus {
    fn of(state: ClassificationState) -> Self {
        Self {
            state,
            version: None,
            message: None,
        }
    }
}

/// Makes a classifier from a verified package (the ONNX one, or a test's).
pub type ClassifierFactory = Arc<dyn Fn(ModelPackage) -> Result<Arc<dyn RoomClassifier>, String> + Send + Sync>;

struct Inner {
    status: ClassificationStatus,
    /// The package folder to load, once one is installed.
    package: Option<PathBuf>,
    classifier: Option<Arc<dyn RoomClassifier>>,
    busy: bool,
}

pub struct RoomClassificationService {
    models_root: Option<PathBuf>,
    factory: Option<ClassifierFactory>,
    inner: Mutex<Inner>,
    /// Classifiers created (tests check that nothing loads early).
    loads: AtomicUsize,
    /// Bumped on every status change, so views can redraw.
    generation: AtomicUsize,
}

impl std::fmt::Debug for RoomClassificationService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoomClassificationService")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

/// The ONNX classifier factory, when the build has one.
pub fn default_factory() -> Option<ClassifierFactory> {
    #[cfg(feature = "classifier")]
    {
        Some(Arc::new(|package: ModelPackage| {
            super::onnx::OnnxRoomClassifier::new(package)
                .map(|c| Arc::new(c) as Arc<dyn RoomClassifier>)
                .map_err(|e| e.0)
        }))
    }
    #[cfg(not(feature = "classifier"))]
    {
        None
    }
}

/// `<data dir>/models/room-classifier`.
pub fn models_root(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join("room-classifier")
}

fn version_key(name: &str) -> (u64, u64, u64) {
    let mut parts = name.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// The newest installed package folder whose manifest names its folder's version (not
/// verified; that happens on load).
fn installed(root: &Path) -> Option<(PathBuf, String)> {
    let mut found: Vec<(PathBuf, String)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .filter_map(|p| {
            let version = ModelPackage::peek(&p)?;
            (p.file_name().is_some_and(|n| n.to_string_lossy() == version)).then_some((p, version))
        })
        .collect();
    found.sort_by_key(|(_, v)| version_key(v));
    found.pop()
}

impl RoomClassificationService {
    /// The service for a data directory. `package_dir` (development) names a package folder to
    /// use as it is. Only manifests are read here.
    pub fn new(data_dir: Option<&Path>, package_dir: Option<PathBuf>) -> Self {
        Self::with_factory(data_dir, package_dir, default_factory())
    }

    pub fn with_factory(
        data_dir: Option<&Path>,
        package_dir: Option<PathBuf>,
        factory: Option<ClassifierFactory>,
    ) -> Self {
        let models_root = data_dir.map(models_root);
        let mut inner = Inner {
            status: ClassificationStatus::of(ClassificationState::NotInstalled),
            package: None,
            classifier: None,
            busy: false,
        };
        if factory.is_none() {
            inner.status = ClassificationStatus::of(ClassificationState::Unavailable);
        } else {
            let found = package_dir
                .and_then(|dir| ModelPackage::peek(&dir).map(|v| (dir, v)))
                .or_else(|| models_root.as_deref().and_then(installed));
            if let Some((dir, version)) = found {
                inner.package = Some(dir);
                inner.status = ClassificationStatus {
                    state: ClassificationState::Ready,
                    version: Some(version),
                    message: None,
                };
            }
        }
        Self {
            models_root,
            factory,
            inner: Mutex::new(inner),
            loads: AtomicUsize::new(0),
            generation: AtomicUsize::new(0),
        }
    }

    /// Ready at once with this classifier (tests and reference scenes).
    pub fn for_testing(classifier: Arc<dyn RoomClassifier>) -> Self {
        let version = classifier.model_version().to_string();
        let shared = classifier.clone();
        let service = Self::with_factory(None, None, Some(Arc::new(move |_| Ok(shared.clone()))));
        {
            let mut inner = service.lock();
            inner.status = ClassificationStatus {
                state: ClassificationState::Ready,
                version: Some(version),
                message: None,
            };
            inner.classifier = Some(classifier);
        }
        service
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set(&self, inner: &mut Inner, status: ClassificationStatus) {
        inner.status = status;
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn status(&self) -> ClassificationStatus {
        self.lock().status.clone()
    }

    /// Changes with every status change.
    pub fn generation(&self) -> usize {
        self.generation.load(Ordering::Relaxed)
    }

    /// The installed package's version, without loading anything (None unless ready).
    pub fn model_version(&self) -> Option<String> {
        let inner = self.lock();
        (inner.status.state == ClassificationState::Ready)
            .then(|| inner.status.version.clone())
            .flatten()
    }

    /// Classifiers created so far.
    pub fn loads(&self) -> usize {
        self.loads.load(Ordering::Relaxed)
    }

    /// The classifier, created on first use: the package is verified and the encoder loaded.
    /// Slow the first time; call it on a worker. `None` unless a package is ready and loads;
    /// a failure moves the service to Failed with the reason.
    pub fn try_get_classifier(&self) -> Option<Arc<dyn RoomClassifier>> {
        let (dir, factory) = {
            let inner = self.lock();
            if let Some(c) = &inner.classifier {
                return Some(c.clone());
            }
            if inner.status.state != ClassificationState::Ready || inner.busy {
                return None;
            }
            (inner.package.clone()?, self.factory.clone()?)
        };
        // Verifying and loading run without the lock, so the status stays readable meanwhile.
        let made = ModelPackage::load(&dir).map_err(|e| e.0).and_then(|p| factory(p));
        let mut inner = self.lock();
        if let Some(c) = &inner.classifier {
            return Some(c.clone());
        }
        match made {
            Ok(classifier) => {
                self.loads.fetch_add(1, Ordering::Relaxed);
                inner.classifier = Some(classifier.clone());
                Some(classifier)
            }
            Err(message) => {
                inner.package = None;
                let status = ClassificationStatus {
                    state: ClassificationState::Failed,
                    version: None,
                    message: Some(message),
                };
                self.set(&mut inner, status);
                None
            }
        }
    }

    /// Install a package from a `.zip` or an extracted package folder. Blocks while it copies
    /// and verifies (run it on a thread); the status says how it went.
    pub fn install_from(&self, source: &Path) {
        let Some(root) = self.models_root.clone() else {
            return;
        };
        {
            let mut inner = self.lock();
            if inner.busy || self.factory.is_none() {
                return;
            }
            inner.busy = true;
            self.set(&mut inner, ClassificationStatus::of(ClassificationState::Installing));
        }
        let result = install(&root, source);
        let mut inner = self.lock();
        inner.busy = false;
        match result {
            Ok(package) => {
                inner.classifier = None;
                inner.package = Some(package.directory.clone());
                let status = ClassificationStatus {
                    state: ClassificationState::Ready,
                    version: Some(package.version),
                    message: None,
                };
                self.set(&mut inner, status);
            }
            Err(e) => {
                inner.package = None;
                inner.classifier = None;
                let status = ClassificationStatus {
                    state: ClassificationState::Failed,
                    version: None,
                    message: Some(e.0),
                };
                self.set(&mut inner, status);
            }
        }
    }
}

/// Copy or extract into a staging folder, verify, then move to `<root>/<version>`.
fn install(root: &Path, source: &Path) -> Result<ModelPackage, PackageError> {
    let fail = |e: std::io::Error| PackageError(e.to_string());
    std::fs::create_dir_all(root).map_err(fail)?;
    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&staging).map_err(fail)?;
    let result = (|| {
        if source.is_dir() {
            copy_package(source, &staging)?;
        } else {
            extract_zip(source, &staging)?;
        }
        let package = ModelPackage::load(&staging)?;
        let destination = root.join(&package.version);
        let previous = destination
            .exists()
            .then(|| root.join(format!(".previous-{}", uuid::Uuid::new_v4().simple())));
        if let Some(previous) = &previous {
            std::fs::rename(&destination, previous).map_err(fail)?;
        }
        if let Err(e) = std::fs::rename(&staging, &destination) {
            if let Some(previous) = &previous {
                let _ = std::fs::rename(previous, &destination);
            }
            return Err(fail(e));
        }
        if let Some(previous) = previous {
            let _ = std::fs::remove_dir_all(previous);
        }
        ModelPackage::load(&destination)
    })();
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

/// The files of a package folder (plain files only, no folders, no hidden names).
fn copy_package(source: &Path, staging: &Path) -> Result<(), PackageError> {
    let fail = |e: std::io::Error| PackageError(e.to_string());
    let mut total = 0u64;
    let mut count = 0;
    for entry in std::fs::read_dir(source).map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type().map_err(fail)?;
        if name.starts_with('.') || !kind.is_file() {
            continue;
        }
        count += 1;
        total += entry.metadata().map_err(fail)?.len();
        if count > MAX_ENTRIES || total > MAX_PACKAGE_BYTES {
            return Err(PackageError("Package exceeds the size limit.".into()));
        }
        std::fs::copy(entry.path(), staging.join(&name)).map_err(fail)?;
    }
    Ok(())
}

#[cfg(feature = "classifier")]
fn extract_zip(source: &Path, staging: &Path) -> Result<(), PackageError> {
    use crate::mudlet::zip::ZipArchive;
    let invalid = || PackageError("Invalid package contents.".into());
    let length = std::fs::metadata(source)
        .map_err(|e| PackageError(e.to_string()))?
        .len();
    if length > MAX_PACKAGE_BYTES {
        return Err(PackageError("Package exceeds the size limit.".into()));
    }
    let data = std::fs::read(source).map_err(|e| PackageError(e.to_string()))?;
    let archive = ZipArchive::open(&data, MAX_ENTRIES).map_err(|_| invalid())?;
    let mut extracted = 0u64;
    for entry in &archive.entries {
        let name = entry.name.as_str();
        if name.is_empty() || name.contains("..") || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(PackageError(format!("Unsafe package entry {name}.")));
        }
        let bytes = archive
            .read(entry, MAX_PACKAGE_BYTES - extracted)
            .map_err(|_| invalid())?;
        extracted += bytes.len() as u64;
        std::fs::write(staging.join(name), bytes).map_err(|e| PackageError(e.to_string()))?;
    }
    Ok(())
}

#[cfg(not(feature = "classifier"))]
fn extract_zip(_: &Path, _: &Path) -> Result<(), PackageError> {
    Err(PackageError("This build cannot read package archives.".into()))
}
