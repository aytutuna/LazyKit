//! The scanner's image work, written once: page clean-up filters, corner decoding and refinement, capture stability.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod background;
mod camera_turn;
mod color;
mod corners;
pub mod detector;
mod detector_lines;
mod detector_regions;
mod enhancer;
pub mod frame_quality;
mod geometry;
mod glare;
mod live;
pub mod mask_ops;
pub mod multi_quad;
mod page_quality;
mod photo;
mod pipeline;
mod refiner;
mod stability;
pub mod warp;

pub use background::{BackgroundMap, percentile};
pub use camera_turn::{frame_candidate, frame_found, photo_outline};
pub use color::is_effectively_gray;
pub use corners::{
    Found, MIN_CONFIDENCE, SIDE, SURE_CONFIDENCE, acceptable, decode_heatmap, model_input,
};
pub use enhancer::{
    DEFAULT_THRESHOLD, EnhanceMode, Enhanced, MAX_PHOTO_GAIN, enhance, is_light_on_dark, paper_map,
    photo, tile_size,
};
pub use geometry::{Point, Quad, aspect};
pub use glare::{
    ANALYSIS_LONG_SIDE as GLARE_ANALYSIS_LONG_SIDE, CLIP_LEVEL as GLARE_CLIP_LEVEL, GlareReading,
    INSET_SHARE as GLARE_INSET_SHARE, MIN_BLOB_SHARE as GLARE_MIN_BLOB_SHARE,
    has_glare as glare_has_glare, measure as glare_measure,
};
pub use lazy_scan_model::{CornerModel, Heatmaps, ModelError};
pub use live::{
    ANALYSIS_LONG_SIDE, CaptureKind, Detection, FrameTicket, HOLD_MS, LiveFrame, LiveObjectSet,
    LiveQuadFilter, LiveQuadTracker, LiveSearch, LiveUpdate, MIN_FRAME_GAP_MS, MIN_OBJECT_GAP_MS,
    OBJECT_OUTLINE_HOLD_MS, ObjectOutput, ObjectReading, PageReading, PageThumbnail, Steadied,
    edge_search,
};
pub use page_quality::{
    ANALYSIS, BLUR_BELOW, CUT_CONFIDENCE, DARK_BELOW, GLARE_FROM, GLARE_UNTIL, Issue, SMALL_BELOW,
    frame_issues, glare_share, mean_luminance, page_issues, sharpness,
};
pub use photo::{
    Action, DpiRule, Filter, ImportPlan, Imported, MAX_DPI, MIN_DPI, NOMINAL_DPI, Orientation,
    Page, PageSizing, Reason, clamped_to_photo, corrected_page, import_photo, model_plan,
    moved_corner, oriented, page_dpi, page_dpi_by_long_side, plan, plan_detection, whole_photo_dpi,
    whole_photo_page,
};
pub use pipeline::{Located, locate};
pub use refiner::{Refinement, build_field, placement, refine};
pub use stability::{
    CapturePlacement, CaptureQuality, CaptureStability, CaptureState, Decision, Observation,
    Options,
};
