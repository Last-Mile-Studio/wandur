//! Room terrain inference (the C# `Wandur.Core.Classification`): a local classifier guesses a
//! room's terrain from its name and description when the world sends none. The guess only
//! colours the map; it never decides identity, exits or routes, and server terrain and manual
//! edits always win.
//!
//! - [`preprocess`]: the text a room is classified on and its inference key.
//! - [`package`]: a classifier package on disk, verified against its manifest.
//! - [`wordpiece`]: the encoder's tokenizer.
//! - `onnx` (feature `classifier`): the package's encoder through `tract-onnx`.
//! - [`service`]: the installed package, its status and the classifier, created lazily.
//! - [`worker`]: classification on a background thread, results handed back to the map.
//!
//! Without the `classifier` feature nothing here loads a model: the service reports that local
//! inference is not part of the build, and the map is unchanged.

// The package, service and ONNX files carry the C# client's English error messages (its
// `InvalidDataException` texts), shown inside the translated "Local model unavailable: {0}";
// their file names are listed as contract files in the app's string check.
#[path = "model_package.rs"]
pub mod package;
pub mod preprocess;
#[path = "classifier_service.rs"]
pub mod service;
pub mod wordpiece;
pub mod worker;

#[cfg(feature = "classifier")]
#[path = "onnx_classifier.rs"]
pub mod onnx;

#[cfg(test)]
mod tests;

pub use package::{ModelPackage, PackageError};
pub use service::{ClassificationState, ClassificationStatus, RoomClassificationService};
pub use worker::{InferenceJob, InferenceResult, InferenceWorker};

/// A terrain guess: a presentation hint only.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomEnvironmentPrediction {
    pub environment: String,
    /// 0 to 1.
    pub confidence: f64,
    pub model_version: String,
}

/// Something that guesses a room's terrain. CPU work: call it off the UI thread.
pub trait RoomClassifier: Send + Sync {
    fn model_version(&self) -> &str;
    fn default_threshold(&self) -> f64;
    /// The best class, or `None` when it scores below `threshold` (an abstention).
    fn classify(
        &self,
        name: &str,
        description: &str,
        threshold: f64,
    ) -> Result<Option<RoomEnvironmentPrediction>, String>;
}

/// The classifier settings' bounds (the C# `ClientSettings` check).
pub const THRESHOLD_RANGE: std::ops::RangeInclusive<f64> = 0.5..=0.99;
/// The default threshold (the 0.1.1 package's own).
pub const DEFAULT_THRESHOLD: f64 = 0.8;

/// Whether this build can run a classifier package (the `classifier` feature).
pub const fn available() -> bool {
    cfg!(feature = "classifier")
}
