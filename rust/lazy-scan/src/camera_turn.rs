use crate::corners::{Found, decode_heatmap, model_input};
use crate::geometry::{Point, Quad};
use crate::live::Detection;
use lazy_image::Rgba;
use lazy_scan_model::CornerModel;

/// A live outline (shares of the sensor's frame, as the frames come) in the photo taken with the turn `rotation_degrees`
/// (clockwise; 0, 90, 180 or 270, the capture's rotation angle), as shares of that photo once it is upright; `None` for any
/// other turn.
pub fn photo_outline(outline: &Quad, rotation_degrees: i32) -> Option<Quad> {
    let turn = rotation_degrees.rem_euclid(360);
    if turn % 90 != 0 {
        return None;
    }
    let corners = outline.0.map(|p| match turn / 90 {
        1 => Point::new(1.0 - p.y, p.x),
        2 => Point::new(1.0 - p.x, 1.0 - p.y),
        3 => Point::new(p.y, 1.0 - p.x),
        _ => p,
    });
    Some(Quad::from_corners(corners))
}

/// The model's page in a live frame as the sensor gives it, as shares of the frame: the frame is turned upright by `turn`
/// degrees (a multiple of 90) for the model, which was trained on pages the right way up, and the corners turned back.
/// `None` when the model found no page or the turn is not a multiple of 90.
pub fn frame_found(frame: &Rgba, turn: i32, model: &dyn CornerModel) -> Option<Found> {
    if turn % 90 != 0 {
        return None;
    }
    let upright = frame.rotated_quarter_turns(turn / 90);
    let heat = model.run(&model_input(&upright)).ok()?;
    let seen = decode_heatmap(&heat.values, heat.width, heat.height)?;
    Some(Found {
        quad: photo_outline(&seen.quad, (360 - turn.rem_euclid(360)) % 360)?,
        confidence: seen.confidence,
    })
}

/// `frame_found` in the frame's pixels, as a page search reports it.
pub fn frame_candidate(frame: &Rgba, turn: i32, model: &dyn CornerModel) -> Option<Detection> {
    let found = frame_found(frame, turn, model)?;
    Some(Detection {
        quad: Some(found.quad.scaled(frame.width as f64, frame.height as f64)),
        confidence: found.confidence,
        cut_off: false,
    })
}
