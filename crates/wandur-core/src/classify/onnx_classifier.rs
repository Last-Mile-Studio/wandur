//! The package's encoder run with `tract-onnx` (pure Rust, no native runtime): one text per
//! run, no padding, as `preprocessing_spec.json` asks. Mean pooling over the tokens, L2
//! normalization, the logistic head (`emb @ coef.T + intercept`) and softmax follow in plain
//! Rust (the C# `OnnxRoomEnvironmentClassifier`).

use tract_onnx::prelude::*;

use super::package::{ModelPackage, PackageError};
use super::wordpiece::WordPiece;
use super::{RoomClassifier, RoomEnvironmentPrediction, preprocess};

type Plan = Arc<TypedRunnableModel>;

pub struct OnnxRoomClassifier {
    package: ModelPackage,
    tokenizer: WordPiece,
    plan: Plan,
    /// The encoder's input names, in its order (`input_ids`, `attention_mask`, and possibly
    /// `token_type_ids`).
    inputs: Vec<String>,
}

impl std::fmt::Debug for OnnxRoomClassifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxRoomClassifier")
            .field("version", &self.package.version)
            .finish_non_exhaustive()
    }
}

impl OnnxRoomClassifier {
    /// Load the encoder and the vocabulary of a verified package. Slow (hundreds of
    /// milliseconds): call it on a worker.
    pub fn new(package: ModelPackage) -> Result<Self, PackageError> {
        let err = |e: TractError| PackageError(format!("encoder.onnx: {e}"));
        let vocabulary = package.vocabulary()?;
        let tokenizer = WordPiece::new(&vocabulary, package.max_word_pieces)
            .ok_or_else(|| PackageError("The vocabulary lacks [CLS], [SEP] or [UNK].".into()))?;
        let mut model = tract_onnx::onnx().model_for_path(package.encoder_path()).map_err(err)?;
        let inputs: Vec<String> = model
            .input_outlets()
            .map_err(err)?
            .iter()
            .map(|o| model.node(o.node).name.clone())
            .collect();
        let sequence = model.symbols.sym("S");
        let fact = InferenceFact::dt_shape(i64::datum_type(), tvec!(1.to_dim(), sequence.to_dim()));
        for index in 0..inputs.len() {
            model = model.with_input_fact(index, fact.clone()).map_err(err)?;
        }
        let plan = model.into_optimized().map_err(err)?.into_runnable().map_err(err)?;
        Ok(Self {
            package,
            tokenizer,
            plan,
            inputs,
        })
    }

    pub fn package(&self) -> &ModelPackage {
        &self.package
    }

    /// The token ids of a classifier text (tests compare them with the fixtures).
    pub fn tokenize(&self, text: &str) -> Vec<u32> {
        self.tokenizer.encode(text)
    }

    /// The sentence embedding: mean over the tokens, L2 normalized.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, String> {
        let ids = self.tokenize(text);
        let n = ids.len();
        let tensor = |values: Vec<i64>| -> Result<TValue, String> {
            Ok(tract_ndarray::Array2::from_shape_vec((1, n), values)
                .map_err(|e| e.to_string())?
                .into_tensor()
                .into_tvalue())
        };
        let mut inputs: TVec<TValue> = tvec!();
        for name in &self.inputs {
            let values = match name.as_str() {
                "input_ids" => ids.iter().map(|&i| i64::from(i)).collect(),
                "attention_mask" => vec![1; n],
                _ => vec![0; n],
            };
            inputs.push(tensor(values)?);
        }
        let outputs = self.plan.run(inputs).map_err(|e| e.to_string())?;
        let hidden = outputs
            .first()
            .ok_or("The encoder gave no output.")?
            .to_plain_array_view::<f32>()
            .map_err(|e| e.to_string())?;
        let shape = hidden.shape();
        if shape.len() != 3 || shape[1] != n || shape[2] != self.package.width {
            return Err(format!("Unexpected encoder output shape {shape:?}."));
        }
        let width = shape[2];
        let flat: Vec<f32> = hidden.iter().copied().collect();
        let mut embedding = vec![0f32; width];
        for token in 0..n {
            for (d, e) in embedding.iter_mut().enumerate() {
                *e += flat[token * width + d];
            }
        }
        let mut norm = 0f64;
        for e in &mut embedding {
            *e /= n as f32;
            norm += f64::from(*e) * f64::from(*e);
        }
        let norm = norm.sqrt();
        if norm > 0.0 {
            for e in &mut embedding {
                *e = (f64::from(*e) / norm) as f32;
            }
        }
        Ok(embedding)
    }

    /// Class probabilities, in the package's class order.
    pub fn probabilities(&self, text: &str) -> Result<Vec<f32>, String> {
        let embedding = self.embed(text)?;
        let width = self.package.width;
        let logits: Vec<f64> = (0..self.package.classes.len())
            .map(|c| {
                let row = &self.package.coefficients[c * width..(c + 1) * width];
                f64::from(self.package.intercepts[c])
                    + row
                        .iter()
                        .zip(&embedding)
                        .map(|(w, e)| f64::from(*w) * f64::from(*e))
                        .sum::<f64>()
            })
            .collect();
        let max = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let exp: Vec<f64> = logits.iter().map(|l| (l - max).exp()).collect();
        let total: f64 = exp.iter().sum();
        Ok(exp.iter().map(|e| (e / total) as f32).collect())
    }
}

impl RoomClassifier for OnnxRoomClassifier {
    fn model_version(&self) -> &str {
        &self.package.version
    }

    fn default_threshold(&self) -> f64 {
        self.package.threshold
    }

    fn classify(
        &self,
        name: &str,
        description: &str,
        threshold: f64,
    ) -> Result<Option<RoomEnvironmentPrediction>, String> {
        let probabilities = self.probabilities(&preprocess::build_text(name, description))?;
        let mut best = 0;
        for (i, p) in probabilities.iter().enumerate() {
            if *p > probabilities[best] {
                best = i;
            }
        }
        let confidence = f64::from(probabilities[best]);
        Ok((confidence >= threshold).then(|| RoomEnvironmentPrediction {
            environment: self.package.classes[best].clone(),
            confidence,
            model_version: self.package.version.clone(),
        }))
    }
}
