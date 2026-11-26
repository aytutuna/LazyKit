//! DocAligner's page corner model behind one trait, with a pure Rust backend for the browser and one on ONNX Runtime for the apps.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use std::fmt;

#[cfg(any(feature = "ort", feature = "ort-linked"))]
mod ort_backend;
#[cfg(feature = "tract")]
mod tract_backend;

#[cfg(any(feature = "ort", feature = "ort-linked"))]
pub use ort_backend::OrtModel;
#[cfg(feature = "tract")]
pub use tract_backend::TractModel;

/// The model takes 3 x 256 x 256 values (blue, green and red planes of 0 to 1).
pub const INPUT_VALUES: usize = 3 * 256 * 256;

/// Four channels of `width` x `height` heat, one per corner (top left, top right, bottom right, bottom left).
#[derive(Clone, Debug)]
pub struct Heatmaps {
    pub values: Vec<f32>,
    pub width: usize,
    pub height: usize,
}

#[derive(Debug)]
pub enum ModelError {
    Load(String),
    Run(String),
    /// The input was not 3 x 256 x 256 values, or the answer not four heatmaps.
    Shape,
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::Load(m) => write!(f, "the corner model did not load: {m}"),
            ModelError::Run(m) => write!(f, "the corner model failed: {m}"),
            ModelError::Shape => write!(f, "the corner model got or gave the wrong shape"),
        }
    }
}

impl std::error::Error for ModelError {}

pub trait CornerModel {
    fn run(&self, input: &[f32]) -> Result<Heatmaps, ModelError>;
}

#[allow(dead_code)]
fn heatmaps(values: Vec<f32>, shape: &[usize]) -> Result<Heatmaps, ModelError> {
    match shape {
        [1, 4, height, width] if values.len() == 4 * width * height => Ok(Heatmaps {
            values,
            width: *width,
            height: *height,
        }),
        _ => Err(ModelError::Shape),
    }
}
