//! The scanner's image work, written once: page clean-up filters, corner decoding and refinement, capture stability.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod background;
mod color;
mod corners;
mod enhancer;
mod geometry;
mod pipeline;
mod refiner;
mod stability;

pub use background::{BackgroundMap, percentile};
pub use color::is_effectively_gray;
pub use corners::{
    Found, MIN_CONFIDENCE, SIDE, SURE_CONFIDENCE, acceptable, decode_heatmap, model_input,
};
pub use enhancer::{
    DEFAULT_THRESHOLD, EnhanceMode, Enhanced, MAX_PHOTO_GAIN, enhance, is_light_on_dark, paper_map,
    photo, tile_size,
};
pub use geometry::{Point, Quad, aspect};
pub use lazy_scan_model::{CornerModel, Heatmaps, ModelError};
pub use pipeline::{Located, locate};
pub use refiner::{Refinement, build_field, placement, refine};
pub use stability::{
    CapturePlacement, CaptureQuality, CaptureStability, CaptureState, Decision, Observation,
    Options,
};
