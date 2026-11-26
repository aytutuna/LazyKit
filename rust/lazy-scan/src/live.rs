use crate::corners::{Found, MIN_CONFIDENCE, acceptable};
use crate::detector;
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

/// A live frame is searched at this long side.
pub const ANALYSIS_LONG_SIDE: usize = 480;

/// The page search the apps hand the tracker: the edge-based detector on the frame (searched at ANALYSIS_LONG_SIDE), with the
/// outline of a moment ago as its hint. A page the frame cuts off is reported as such when the detector saw its outline.
pub fn edge_search(image: &Rgba, hint: Option<&Quad>) -> Detection {
    let options = detector::Options {
        analysis_long_side: ANALYSIS_LONG_SIDE,
        ..Default::default()
    };
    let found = detector::detect(image, hint, &options);
    Detection {
        quad: found.quad,
        confidence: found.confidence,
        cut_off: found.rejection == detector::Rejection::CutOff && found.candidate.is_some(),
    }
}

/// Which things a shot looks for; what the object search is told so its finder knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    Document,
    Receipt,
    Photo,
    IdCard,
}

/// What the live frames are searched for: the page of a document or card, every object of a kind with several to a shot
/// (receipts, photos), or nothing while the phone is too warm or saving power for the object search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveSearch {
    Page,
    Objects(CaptureKind),
    Off,
}

/// A frame this soon after the last one searched for objects is passed over (about four searches a second).
pub const MIN_OBJECT_GAP_MS: f64 = 225.0;
/// A lost set keeps its outlines this long.
pub const OBJECT_OUTLINE_HOLD_MS: f64 = 500.0;

/// What `LiveObjectSet::update` says of a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectOutput {
    pub shown: Vec<Quad>,
    pub state: CaptureState,
    pub progress: f64,
    pub is_steady: bool,
    pub should_capture: bool,
}

/// Steadies the objects found in successive live frames (pixels of one frame size) as a set, for the shutter and the
/// outlines. The set holds still while it has the same number of objects and each stays within `stay_movement` of the
/// diagonal of where it was when the set last changed; the outlines drawn are then the ones from that moment, so they do not
/// tremble. The shutter fires once the set has held still for `steady_ms` on a sharp, lit frame and does not fire again
/// until the set changes (an object `new_page_movement` away, another count) or is gone for HOLD_MS. A set lost keeps its
/// outlines for OBJECT_OUTLINE_HOLD_MS. One frame after another in time order.
#[derive(Clone, Debug)]
pub struct LiveObjectSet {
    options: Options,
    anchor: Vec<Quad>,
    steady_from: f64,
    shown: Vec<Quad>,
    shown_ms: f64,
    taken: Option<Vec<Quad>>,
    missing_from: Option<f64>,
}

impl LiveObjectSet {
    pub fn new(options: Options) -> Self {
        LiveObjectSet {
            options,
            anchor: Vec::new(),
            steady_from: 0.0,
            shown: Vec::new(),
            shown_ms: f64::NEG_INFINITY,
            taken: None,
            missing_from: None,
        }
    }

    pub fn is_taken(&self) -> bool {
        self.taken.is_some()
    }

    /// The set found in the frame taken at `t` ms, in a frame whose diagonal is `diagonal` px.
    pub fn update(
        &mut self,
        found: &[Quad],
        t: f64,
        diagonal: f64,
        quality: CaptureQuality,
    ) -> ObjectOutput {
        let o = self.options;
        if found.is_empty() {
            self.anchor.clear();
            let since = *self.missing_from.get_or_insert(t);
            if t - since > HOLD_MS {
                self.taken = None;
            }
            if t < self.shown_ms || t - self.shown_ms > OBJECT_OUTLINE_HOLD_MS {
                self.shown.clear();
            }
            return ObjectOutput {
                shown: self.shown.clone(),
                state: CaptureState::NoPage,
                progress: 0.0,
                is_steady: false,
                should_capture: false,
            };
        }
        self.missing_from = None;
        if let Some(held) = &self.taken {
            if Self::matches(found, held, o.new_page_movement * diagonal) {
                let still = Self::matches(found, held, o.stay_movement * diagonal);
                self.shown = found.to_vec();
                self.shown_ms = t;
                return ObjectOutput {
                    shown: self.shown.clone(),
                    state: CaptureState::Captured,
                    progress: 1.0,
                    is_steady: still,
                    should_capture: false,
                };
            }
            self.taken = None;
        }
        let steady = Self::matches(found, &self.anchor, o.stay_movement * diagonal);
        if !steady {
            self.anchor = found.to_vec();
            self.steady_from = t;
        }
        self.shown = self.anchor.clone();
        self.shown_ms = t;
        let held = t - self.steady_from;
        let progress = if steady {
            (held / o.steady_ms).min(1.0)
        } else {
            0.0
        };
        if steady && held >= o.steady_ms && quality == CaptureQuality::Good {
            self.taken = Some(self.anchor.clone());
            return ObjectOutput {
                shown: self.anchor.clone(),
                state: CaptureState::Captured,
                progress: 1.0,
                is_steady: true,
                should_capture: true,
            };
        }
        ObjectOutput {
            shown: self.anchor.clone(),
            state: if steady {
                CaptureState::Steadying
            } else {
                CaptureState::Moving
            },
            progress,
            is_steady: steady,
            should_capture: false,
        }
    }

    /// The set in view counts as taken by hand.
    pub fn mark_captured(&mut self) {
        if !self.anchor.is_empty() {
            self.taken = Some(self.anchor.clone());
        }
    }

    /// The camera was stopped for a while: that time is neither time the set held still nor time it was missing.
    pub fn resume(&mut self, t: f64) {
        self.steady_from = t;
        self.missing_from = None;
        self.shown_ms = t;
    }

    /// Judges the set afresh, keeping the outlines drawn.
    pub fn rejudge(&mut self) {
        self.anchor.clear();
        self.taken = None;
        self.missing_from = None;
    }

    /// `a` and `b` have as many objects, each of `a` within `tolerance` px of one of `b` (corner by corner, whichever corner
    /// each names first).
    pub fn matches(a: &[Quad], b: &[Quad], tolerance: f64) -> bool {
        if b.is_empty() || a.len() != b.len() {
            return false;
        }
        let mut free: Vec<&Quad> = b.iter().collect();
        for quad in a {
            let mut index = None;
            let mut best = tolerance;
            for (i, other) in free.iter().enumerate() {
                let distance = aligned(quad, other).max_corner_distance(other);
                if distance <= best {
                    best = distance;
                    index = Some(i);
                }
            }
            let Some(at) = index else { return false };
            free.remove(at);
        }
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
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
    /// The outlines of every object found, in 0 to 1 of the frame, for the object search; empty for the page search, where
    /// `outline` is the one.
    pub outlines: Vec<Quad>,
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
    search: LiveSearch,
    objects: LiveObjectSet,
    largest_only: bool,
    reduced: bool,
    generation: u64,
}

impl Default for LiveQuadTracker {
    fn default() -> Self {
        Self::new(Options::default())
    }
}

/// A frame accepted by `LiveQuadTracker::begin`, with what the search needs to know. The search can run without the tracker
/// (on another thread, say); `finish_page` and `finish_objects` then drop its answer when the tracker was reset or its search
/// changed meanwhile.
#[derive(Clone, Copy, Debug)]
pub struct FrameTicket {
    generation: u64,
    search: LiveSearch,
    hint: Option<Quad>,
    model_lately: bool,
    options: Options,
}

/// What searching a frame for its page found.
#[derive(Clone, Copy, Debug)]
pub struct PageReading {
    detection: Detection,
    model_placed: bool,
    reading: frame_quality::Reading,
}

/// What searching a frame for its objects found.
#[derive(Clone, Debug)]
pub struct ObjectReading {
    found: Vec<Quad>,
    quality: CaptureQuality,
}

impl FrameTicket {
    pub fn search(&self) -> LiveSearch {
        self.search
    }

    /// The page in `frame`: the model's when it is sure enough, else nothing while the model has had it lately, else what
    /// `search` finds (it gets the page of a moment ago as a hint), then the frame measured for sharpness and light.
    pub fn read_page(
        &self,
        frame: &LiveFrame,
        search: impl FnOnce(&Rgba, Option<&Quad>) -> Detection,
    ) -> PageReading {
        let (w, h) = (frame.image.width as f64, frame.image.height as f64);
        let floor = MIN_CONFIDENCE.max(self.options.enter_confidence);
        let seen = frame
            .model
            .filter(|m| m.confidence >= floor)
            .and_then(|m| LiveQuadTracker::model_page(&m, w, h).map(|quad| (quad, m.confidence)));
        let detection = match seen {
            Some((quad, confidence)) => Detection {
                quad: Some(quad),
                confidence,
                cut_off: false,
            },
            None if self.model_lately => Detection::default(),
            None => search(&frame.image, self.hint.as_ref()),
        };
        PageReading {
            detection,
            model_placed: seen.is_some(),
            reading: frame_quality::measure(&frame.image, detection.quad.as_ref()),
        }
    }

    /// The objects of the ticket's kind in `frame`, the frame measured for sharpness and light. Nothing is found when the
    /// ticket is not for the object search.
    pub fn read_objects(
        &self,
        frame: &LiveFrame,
        find: impl FnOnce(&Rgba, CaptureKind) -> Vec<Quad>,
    ) -> ObjectReading {
        let LiveSearch::Objects(kind) = self.search else {
            return ObjectReading {
                found: Vec::new(),
                quality: CaptureQuality::Good,
            };
        };
        let found = find(&frame.image, kind);
        let reading = frame_quality::measure(&frame.image, None);
        ObjectReading {
            found,
            quality: CaptureStability::judge(
                Some(reading.sharpness),
                Some(reading.brightness),
                &self.options,
            ),
        }
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
            search: LiveSearch::Page,
            objects: LiveObjectSet::new(options),
            largest_only: false,
            reduced: false,
            generation: 0,
        }
    }

    /// What the frames are searched for from now on; a change judges everything afresh.
    pub fn set_search(&mut self, next: LiveSearch) {
        if next == self.search {
            return;
        }
        self.search = next;
        self.clear_state();
        self.generation += 1;
    }

    /// While the phone is warm the page is searched as seldom as objects are (MIN_OBJECT_GAP_MS).
    pub fn set_reduced(&mut self, on: bool) {
        self.reduced = on;
    }

    /// Only the largest object found is kept, as a shot with Items in frame set to One keeps it.
    pub fn set_largest_only(&mut self, on: bool) {
        self.largest_only = on;
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

    /// Takes in a frame: `None` while the search is off or when the frame came too soon after the last one searched. A frame
    /// from an earlier time than the last one (the camera started again) or of another size starts over.
    pub fn begin(&mut self, frame: &LiveFrame) -> Option<FrameTicket> {
        let t = frame.timestamp_ms;
        let (width, height) = (frame.image.width, frame.image.height);
        if self.search == LiveSearch::Off {
            return None;
        }
        let gap = if self.search != LiveSearch::Page || self.reduced {
            MIN_OBJECT_GAP_MS
        } else {
            MIN_FRAME_GAP_MS
        };
        if let Some(last) = self.last_searched {
            if t < last {
                self.clear_state();
                self.generation += 1;
            } else if t - last < gap {
                return None;
            }
        }
        if (width, height) != self.frame_size {
            self.clear_state();
            self.generation += 1;
            self.frame_size = (width, height);
        }
        self.last_searched = Some(t);
        Some(FrameTicket {
            generation: self.generation,
            search: self.search,
            hint: self
                .last_quad
                .filter(|_| t >= self.last_seen && t - self.last_seen <= HOLD_MS),
            model_lately: t >= self.model_seen && t - self.model_seen <= HOLD_MS,
            options: self.options,
        })
    }

    /// Reads one frame. `search` is the page search (edges) for frames the model did not place; it gets the page of a
    /// moment ago as a hint. `None` when the frame came too soon after the last one. This reads a frame for the page only:
    /// while the search is for objects, use `process_with`.
    pub fn process(
        &mut self,
        frame: &LiveFrame,
        search: impl FnOnce(&Rgba, Option<&Quad>) -> Detection,
    ) -> Option<LiveUpdate> {
        self.process_with(frame, search, |_, _| Vec::new())
    }

    /// Reads one frame for whatever is searched: the page (with `search`) or the objects of a kind (with `find`).
    pub fn process_with(
        &mut self,
        frame: &LiveFrame,
        search: impl FnOnce(&Rgba, Option<&Quad>) -> Detection,
        find: impl FnOnce(&Rgba, CaptureKind) -> Vec<Quad>,
    ) -> Option<LiveUpdate> {
        let ticket = self.begin(frame)?;
        match ticket.search {
            LiveSearch::Objects(_) => {
                let read = ticket.read_objects(frame, find);
                self.finish_objects(&ticket, frame, read)
            }
            _ => {
                let read = ticket.read_page(frame, search);
                self.finish_page(&ticket, frame, read)
            }
        }
    }

    /// Judges a frame whose page was searched. `None` when the tracker was reset after `begin`: the answer belongs to before.
    pub fn finish_page(
        &mut self,
        ticket: &FrameTicket,
        frame: &LiveFrame,
        read: PageReading,
    ) -> Option<LiveUpdate> {
        if ticket.generation != self.generation {
            return None;
        }
        let t = frame.timestamp_ms;
        let (width, height) = (frame.image.width, frame.image.height);
        let (w, h) = (width as f64, height as f64);
        let PageReading {
            detection,
            model_placed,
            reading,
        } = read;

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
        if model_placed {
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
            outlines: Vec::new(),
        })
    }

    /// Steadies the objects found in a frame as a set. `None` when the tracker was reset or its search changed after
    /// `begin`.
    pub fn finish_objects(
        &mut self,
        ticket: &FrameTicket,
        frame: &LiveFrame,
        read: ObjectReading,
    ) -> Option<LiveUpdate> {
        if ticket.generation != self.generation {
            return None;
        }
        let t = frame.timestamp_ms;
        let (width, height) = (frame.image.width, frame.image.height);
        let ObjectReading { mut found, quality } = read;
        if self.largest_only
            && let Some(largest) = found.iter().copied().reduce(|best, q| {
                // the first of equal areas stays, as Swift's max(by:) keeps it
                if q.area() > best.area() { q } else { best }
            })
        {
            found = vec![largest];
        }
        if self.resuming {
            self.resuming = false;
            self.objects.resume(t);
        }
        let (w, h) = (width as f64, height as f64);
        let diagonal = (w * w + h * h).sqrt();
        let seen = self.objects.update(&found, t, diagonal, quality);
        if !self.objects.is_taken() {
            self.photographed = false;
        }
        let outlines = seen
            .shown
            .iter()
            .map(|q| q.scaled(1.0 / w.max(1.0), 1.0 / h.max(1.0)))
            .collect();
        Some(LiveUpdate {
            outline: None,
            state: seen.state,
            progress: seen.progress,
            timestamp_ms: t,
            is_steady: seen.is_steady,
            should_capture: seen.should_capture,
            quality,
            placement: CapturePlacement::Fine,
            suggests_shutter: false,
            outlines,
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
        self.generation += 1;
    }

    /// A photo was taken by hand: the page in view counts as taken, so the next is taken only once it moved away or went.
    pub fn mark_captured(&mut self) {
        self.stability.mark_captured();
        self.objects.mark_captured();
        if self.stability.state() == CaptureState::Captured {
            self.held_at = self.last_found;
            self.photographed = true;
            self.photographed_look = None;
            if let (Some(image), Some(page)) = (&self.latest, &self.last_found) {
                self.photographed_look = Some(PageThumbnail::new(image, Some(page)));
            }
        } else if self.objects.is_taken() {
            self.photographed = true;
        }
    }

    /// Judges again a page decided on without a photo (the photo failed); a page photographed stays taken.
    pub fn rearm(&mut self) {
        let untaken = self.stability.state() == CaptureState::Captured || self.objects.is_taken();
        if untaken && !self.photographed {
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
        self.objects = LiveObjectSet::new(self.options);
    }

    fn clear_judgement(&mut self) {
        self.stability = CaptureStability::new(self.options);
        self.last_searched = None;
        self.last_found = None;
        self.held_at = None;
        self.resuming = false;
        self.photographed = false;
        self.photographed_look = None;
        self.objects.rejudge();
    }
}
