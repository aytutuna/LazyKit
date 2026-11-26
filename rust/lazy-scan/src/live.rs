use crate::corners::{Found, MIN_CONFIDENCE, acceptable};
use crate::frame_quality;
use crate::geometry::{Point, Quad};
use crate::stability::{
    CapturePlacement, CaptureQuality, CaptureStability, CaptureState, Observation, Options,
};
use lazy_image::{Rgba, luma};
use std::sync::Arc;

/// A page missed for this long is gone: its outline stays up until then.
pub const HOLD_MS: f64 = 300.0;
/// Frames closer together than this are passed over before any work.
pub const MIN_FRAME_GAP_MS: f64 = 90.0;

pub const SMOOTHING: f64 = 0.5;
pub const JITTER_SHARE: f64 = 0.01;
pub const JUMP_SHARE: f64 = 0.06;

/// The corners of `quad` renamed, by starting from whichever one lies nearest `reference`'s first, so they pair with it.
fn aligned(quad: &Quad, reference: &Quad) -> Quad {
    let (mut best_shift, mut best) = (0, f64::MAX);
    for shift in 0..4 {
        let worst = (0..4)
            .map(|i| quad.0[(i + shift) & 3].distance_to(reference.0[i]))
            .fold(0.0, f64::max);
        if worst < best {
            best = worst;
            best_shift = shift;
        }
    }
    Quad(std::array::from_fn(|i| quad.0[(i + best_shift) & 3]))
}

fn blended(before: &Quad, found: &Quad, jitter: f64) -> Quad {
    Quad(std::array::from_fn(|i| {
        let (a, b) = (before.0[i], found.0[i]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        if (dx * dx + dy * dy).sqrt() <= jitter {
            Point::new(a.x + SMOOTHING * dx, a.y + SMOOTHING * dy)
        } else {
            b
        }
    }))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Steadied {
    /// What the shutter judges; `None` while the page is not believed.
    pub page: Option<Quad>,
    /// What is drawn.
    pub shown: Option<Quad>,
}

/// Steadies the page found in successive live frames (pixels of one frame size) before it is drawn and judged. A corner
/// that moved at most JITTER_SHARE of the diagonal since the outline before moves only SMOOTHING of the way; one that moved
/// further is followed at once. A page that jumped more than JUMP_SHARE is believed only when the next frame finds it away
/// from the outline too. A page seen for the first time is drawn from its second sighting, and a page drawn and then missed
/// keeps its outline for HOLD_MS after it was last found.
#[derive(Clone, Debug)]
pub struct LiveQuadFilter {
    current: Option<Quad>,
    drawn: bool,
    jumped: Option<Quad>,
    seen_ms: f64,
}

impl Default for LiveQuadFilter {
    fn default() -> Self {
        LiveQuadFilter {
            current: None,
            drawn: false,
            jumped: None,
            seen_ms: f64::NEG_INFINITY,
        }
    }
}

impl LiveQuadFilter {
    pub fn update(&mut self, found: Option<Quad>, t: f64, diagonal: f64) -> Steadied {
        if self.current.is_some() && (t - self.seen_ms > HOLD_MS || t < self.seen_ms) {
            self.current = None;
            self.drawn = false;
            self.jumped = None;
        }
        let Some(quad) = found else {
            self.jumped = None;
            if !self.drawn {
                self.current = None;
            }
            return Steadied {
                page: None,
                shown: if self.drawn { self.current } else { None },
            };
        };
        let Some(before) = self.current else {
            self.current = Some(quad);
            self.seen_ms = t;
            return Steadied {
                page: Some(quad),
                shown: None,
            };
        };
        let aligned = aligned(&quad, &before);
        if aligned.max_corner_distance(&before) > JUMP_SHARE * diagonal {
            if !self.drawn {
                self.current = Some(quad);
                self.seen_ms = t;
                return Steadied {
                    page: Some(quad),
                    shown: None,
                };
            }
            if self.jumped.is_some() {
                self.current = Some(quad);
                self.jumped = Some(quad);
                self.seen_ms = t;
                return Steadied {
                    page: Some(quad),
                    shown: Some(quad),
                };
            }
            self.jumped = Some(quad);
            return Steadied {
                page: None,
                shown: Some(before),
            };
        }
        let steadied = blended(&before, &aligned, JITTER_SHARE * diagonal);
        self.current = Some(steadied);
        self.drawn = true;
        self.jumped = None;
        self.seen_ms = t;
        Steadied {
            page: Some(steadied),
            shown: Some(steadied),
        }
    }
}

/// A coarse look at the page and what surrounds it, to tell the page already taken from another.
#[derive(Clone, Debug, PartialEq)]
pub struct PageThumbnail {
    cells: Vec<f64>,
    surround: Option<f64>,
}

impl PageThumbnail {
    const SIDE: usize = 24;
    const MIN_CORRELATION: f64 = 0.9;
    const FLAT_SPREAD: f64 = 4.0;
    const FLAT_BRIGHTNESS: f64 = 12.0;
    const FLAT_RATIO: f64 = 0.1;
    const MIN_SURROUND: f64 = 0.05;
    const MIN_SURROUND_LUMA: f64 = 16.0;

    pub fn new(frame: &Rgba, page: Option<&Quad>) -> Self {
        let gray = luma(frame);
        let (w, h) = (gray.width, gray.height);
        let b = frame_quality::area(page, w, h);
        let outside = w * h - (b.x1 - b.x0) * (b.y1 - b.y0);
        let mut inside = 0u64;
        for y in b.y0..b.y1 {
            inside += gray.data[y * w + b.x0..y * w + b.x1]
                .iter()
                .map(|&v| v as u64)
                .sum::<u64>();
        }
        let total: u64 = gray.data.iter().map(|&v| v as u64).sum();
        let around = (total - inside) as f64 / outside.max(1) as f64;
        let surround = (outside as f64 >= Self::MIN_SURROUND * (w * h) as f64
            && around >= Self::MIN_SURROUND_LUMA)
            .then_some(around);
        let (bw, bh) = (b.x1 - b.x0, b.y1 - b.y0);
        let side = Self::SIDE;
        let mut cells = Vec::new();
        for cy in 1..side - 1 {
            let y0 = b.y0 + cy * bh / side;
            let y1 = (b.y0 + (cy + 1) * bh / side).max(y0 + 1).min(b.y1);
            for cx in 1..side - 1 {
                let x0 = b.x0 + cx * bw / side;
                let x1 = (b.x0 + (cx + 1) * bw / side).max(x0 + 1).min(b.x1);
                let mut sum = 0u64;
                for y in y0..y1 {
                    sum += gray.data[y * w + x0..y * w + x1]
                        .iter()
                        .map(|&v| v as u64)
                        .sum::<u64>();
                }
                cells.push(sum as f64 / ((y1 - y0) * (x1 - x0)).max(1) as f64);
            }
        }
        PageThumbnail { cells, surround }
    }

    pub fn matches(&self, other: &PageThumbnail) -> bool {
        let n = self.cells.len() as f64;
        let mean_a = self.cells.iter().sum::<f64>() / n;
        let mean_b = other.cells.iter().sum::<f64>() / n;
        let (mut dot, mut spread_a, mut spread_b) = (0.0, 0.0, 0.0);
        for (a, b) in self.cells.iter().zip(&other.cells) {
            let (a, b) = (a - mean_a, b - mean_b);
            dot += a * b;
            spread_a += a * a;
            spread_b += b * b;
        }
        let flat = Self::FLAT_SPREAD * Self::FLAT_SPREAD * n;
        if spread_a < flat && spread_b < flat {
            if let (Some(a), Some(b)) = (self.surround, other.surround)
                && a > 0.0
                && b > 0.0
            {
                let (ratio_a, ratio_b) = (mean_a / a, mean_b / b);
                return (ratio_a - ratio_b).abs() <= Self::FLAT_RATIO * ratio_a.max(ratio_b);
            }
            return (mean_a - mean_b).abs() <= Self::FLAT_BRIGHTNESS;
        }
        if spread_a < flat || spread_b < flat {
            return false;
        }
        dot / (spread_a * spread_b).sqrt() >= Self::MIN_CORRELATION
    }
}

/// One camera frame: the photo the page search reads (small, in the display's orientation), when it was taken and, if the
/// corner model ran on it, what the model found.
pub struct LiveFrame {
    pub image: Arc<Rgba>,
    pub timestamp_ms: f64,
    pub model: Option<Found>,
}

/// What a page search found in a frame, for the frame's pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Detection {
    pub quad: Option<Quad>,
    pub confidence: f64,
    /// A page was seen but the frame cuts it off.
    pub cut_off: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveUpdate {
    /// The page to draw, in 0 to 1 of the frame.
    pub outline: Option<Quad>,
    pub state: CaptureState,
    pub progress: f64,
    pub timestamp_ms: f64,
    pub is_steady: bool,
    pub should_capture: bool,
    pub quality: CaptureQuality,
    pub placement: CapturePlacement,
    pub suggests_shutter: bool,
}

/// Follows the page through a camera's frames: the model's page (or the caller's search while the model has not had it for
/// HOLD_MS), steadied by the filter, measured for sharpness and light, and judged by CaptureStability. Once the page is
/// taken it stays taken until it moves away, and the page that comes back after a shake is told from another by its
/// thumbnail. One frame after another, from one thread; the caller serialises.
pub struct LiveQuadTracker {
    options: Options,
    stability: CaptureStability,
    last_searched: Option<f64>,
    last_quad: Option<Quad>,
    last_seen: f64,
    filter: LiveQuadFilter,
    model_seen: f64,
    last_found: Option<Quad>,
    held_at: Option<Quad>,
    frame_size: (usize, usize),
    resuming: bool,
    photographed: bool,
    latest: Option<Arc<Rgba>>,
    photographed_look: Option<PageThumbnail>,
}

impl Default for LiveQuadTracker {
    fn default() -> Self {
        Self::new(Options::default())
    }
}

impl LiveQuadTracker {
    pub fn new(options: Options) -> Self {
        LiveQuadTracker {
            options,
            stability: CaptureStability::new(options),
            last_searched: None,
            last_quad: None,
            last_seen: f64::NEG_INFINITY,
            filter: LiveQuadFilter::default(),
            model_seen: f64::NEG_INFINITY,
            last_found: None,
            held_at: None,
            frame_size: (0, 0),
            resuming: false,
            photographed: false,
            latest: None,
            photographed_look: None,
        }
    }

    /// The model's page in the frame's pixels, when it is sure enough to be taken and a document's shape, its corners
    /// named from the top left as the apps' MultiQuadPicker does, kept within the frame and still cut along (the apps'
    /// canUseCorners).
    fn model_page(found: &Found, w: f64, h: f64) -> Option<Quad> {
        if !acceptable(found, w, h) {
            return None;
        }
        let clamped = Quad::from_corners(found.quad.scaled(w, h).0).clamped_to_photo(w, h);
        clamped.usable().then_some(clamped)
    }

    /// Reads one frame. `search` is the page search (edges) for frames the model did not place; it gets the page of a
    /// moment ago as a hint. `None` when the frame came too soon after the last one.
    pub fn process(
        &mut self,
        frame: &LiveFrame,
        search: impl FnOnce(&Rgba, Option<&Quad>) -> Detection,
    ) -> Option<LiveUpdate> {
        let t = frame.timestamp_ms;
        let (width, height) = (frame.image.width, frame.image.height);
        if let Some(last) = self.last_searched {
            if t < last {
                self.clear_state();
            } else if t - last < MIN_FRAME_GAP_MS {
                return None;
            }
        }
        if (width, height) != self.frame_size {
            self.clear_state();
            self.frame_size = (width, height);
        }
        self.last_searched = Some(t);
        let hint = self
            .last_quad
            .filter(|_| t >= self.last_seen && t - self.last_seen <= HOLD_MS);
        let model_lately = t >= self.model_seen && t - self.model_seen <= HOLD_MS;

        let (w, h) = (width as f64, height as f64);
        let floor = MIN_CONFIDENCE.max(self.options.enter_confidence);
        let seen = frame
            .model
            .filter(|m| m.confidence >= floor)
            .and_then(|m| Self::model_page(&m, w, h).map(|quad| (quad, m.confidence)));
        let detection = match seen {
            Some((quad, confidence)) => Detection {
                quad: Some(quad),
                confidence,
                cut_off: false,
            },
            None if model_lately => Detection::default(),
            None => search(&frame.image, hint.as_ref()),
        };
        let reading = frame_quality::measure(&frame.image, detection.quad.as_ref());

        if self.resuming {
            // the time the camera was stopped is not time without the page
            self.resuming = false;
            self.stability.resume(t);
        }
        let diagonal = (w * w + h * h).sqrt();
        let steadied = self.filter.update(detection.quad, t, diagonal);
        let observation = Observation {
            quad: steadied.page,
            confidence: detection.confidence,
            timestamp_ms: t,
            frame_width: width as u32,
            frame_height: height as u32,
            sharpness: Some(reading.sharpness),
            brightness: Some(reading.brightness),
        };
        let mut decision = self.stability.update(&observation);
        self.latest = Some(Arc::clone(&frame.image));
        // the page came back after a shake or a hand over it and held still: it is the page already taken
        if decision.should_capture
            && let (Some(look), Some(page)) = (&self.photographed_look, &steadied.page)
            && PageThumbnail::new(&frame.image, Some(page)).matches(look)
        {
            decision.should_capture = false;
            self.photographed = true;
        }
        if decision.state != CaptureState::Captured {
            self.photographed = false;
        }
        let steady = self.holds_still(&decision, steadied.page, detection.confidence, diagonal);
        self.last_found = steadied.page;
        if seen.is_some() {
            self.model_seen = t;
        }
        if let Some(quad) = detection.quad {
            self.last_quad = Some(quad);
            self.last_seen = t;
        } else if t - self.last_seen > HOLD_MS {
            self.last_quad = None;
        }
        let outline = steadied
            .shown
            .map(|q| q.scaled(1.0 / w.max(1.0), 1.0 / h.max(1.0)));
        let placement = match steadied.page {
            Some(page) => CaptureStability::placement(&page, width as u32, height as u32),
            None if detection.cut_off => CapturePlacement::CutOff,
            None => CapturePlacement::Fine,
        };
        Some(LiveUpdate {
            outline,
            state: decision.state,
            progress: decision.progress,
            timestamp_ms: t,
            is_steady: steady,
            should_capture: decision.should_capture,
            quality: decision.quality,
            placement,
            suggests_shutter: decision.suggests_shutter,
        })
    }

    // Once taken, the page keeps the captured state, so whether it holds still meanwhile is judged here by the same rules.
    fn holds_still(
        &mut self,
        decision: &crate::stability::Decision,
        quad: Option<Quad>,
        confidence: f64,
        diagonal: f64,
    ) -> bool {
        let o = self.options;
        match decision.state {
            CaptureState::Steadying => {
                self.held_at = None;
                true
            }
            CaptureState::NoPage | CaptureState::Moving => {
                self.held_at = None;
                false
            }
            CaptureState::Captured => {
                let Some(found) = quad else {
                    return self.held_at.is_some();
                };
                if decision.should_capture {
                    self.held_at = Some(found);
                    return true;
                }
                if let Some(held) = self.held_at
                    && confidence >= o.stay_confidence
                    && found.movement(&held) <= o.stay_movement * diagonal
                {
                    return true;
                }
                if let Some(earlier) = self.last_found
                    && confidence >= o.enter_confidence
                    && found.movement(&earlier) <= o.enter_movement * diagonal
                {
                    self.held_at = Some(earlier);
                    return true;
                }
                self.held_at = None;
                false
            }
        }
    }

    /// Forgets the page: the next one that holds still is taken.
    pub fn reset(&mut self) {
        self.clear_judgement();
    }

    /// A photo was taken by hand: the page in view counts as taken, so the next is taken only once it moved away or went.
    pub fn mark_captured(&mut self) {
        self.stability.mark_captured();
        if self.stability.state() == CaptureState::Captured {
            self.held_at = self.last_found;
            self.photographed = true;
            self.photographed_look = None;
            if let (Some(image), Some(page)) = (&self.latest, &self.last_found) {
                self.photographed_look = Some(PageThumbnail::new(image, Some(page)));
            }
        }
    }

    /// Judges again a page decided on without a photo (the photo failed); a page photographed stays taken.
    pub fn rearm(&mut self) {
        if self.stability.state() == CaptureState::Captured && !self.photographed {
            self.reset();
        }
    }

    /// The camera stopped (the corner editor, the background): the time until the next frame counts neither as time the page
    /// was missing nor as time it held still, and a page taken stays taken.
    pub fn pause(&mut self) {
        self.resuming = true;
    }

    fn clear_state(&mut self) {
        self.clear_judgement();
        self.last_quad = None;
        self.last_seen = f64::NEG_INFINITY;
        self.filter = LiveQuadFilter::default();
        self.model_seen = f64::NEG_INFINITY;
    }

    fn clear_judgement(&mut self) {
        self.stability = CaptureStability::new(self.options);
        self.last_searched = None;
        self.last_found = None;
        self.held_at = None;
        self.resuming = false;
        self.photographed = false;
        self.photographed_look = None;
    }
}
