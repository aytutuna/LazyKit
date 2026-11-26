use crate::geometry::Quad;

// Decides when the camera may take the page by itself, as Lazy Scan's CaptureStability does: the page must be seen with
// confidence for STEADY_MS with its corners holding still. Hysteresis keeps the decision from flickering: holding still
// starts when no corner moved more than ENTER_MOVEMENT of the diagonal since the previous frame and the confidence is at
// least ENTER_CONFIDENCE, and lasts while no corner strays more than STAY_MOVEMENT from where it was then and the
// confidence stays at least STAY_CONFIDENCE. A page missing for up to MAX_GAP_MS is waited for, and the time it was
// missing does not count as holding still. Once taken, the same page is not taken again until it goes (frames without a
// page spanning more than MAX_GAP_MS) or a corner moves NEW_PAGE_MOVEMENT from where it was taken.
pub const STEADY_MS: f64 = 600.0;
pub const ENTER_MOVEMENT: f64 = 0.01;
pub const STAY_MOVEMENT: f64 = 0.025;
pub const ENTER_CONFIDENCE: f64 = 0.6;
pub const STAY_CONFIDENCE: f64 = 0.45;
pub const MAX_GAP_MS: f64 = 250.0;
pub const NEW_PAGE_MOVEMENT: f64 = 0.15;
pub const MIN_SHARPNESS: f64 = 40.0;
pub const MIN_BRIGHTNESS: f64 = 50.0;
pub const MIN_AREA_SHARE: f64 = 0.2;
pub const EDGE_MARGIN: f64 = 0.01;
pub const SOFT_HINT_MS: f64 = 2000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureState {
    NoPage,
    Moving,
    Steadying,
    Captured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureQuality {
    Good,
    Blurry,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturePlacement {
    Fine,
    Far,
    CutOff,
}

/// One video frame's detection: the page in the frame's pixels, the confidence, when the frame was taken (milliseconds on
/// any steady clock) and the frame's size, which movement is measured against.
#[derive(Clone, Copy, Debug)]
pub struct Observation {
    pub quad: Option<Quad>,
    pub confidence: f64,
    pub timestamp_ms: f64,
    pub frame_width: u32,
    pub frame_height: u32,
    pub sharpness: Option<f64>,
    pub brightness: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Decision {
    pub state: CaptureState,
    pub should_capture: bool,
    /// How far the page is through the steady time, 0 to 1.
    pub progress: f64,
    pub quality: CaptureQuality,
    /// A page held still has waited SOFT_HINT_MS for a sharp frame: tapping the shutter may do better.
    pub suggests_shutter: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub steady_ms: f64,
    pub enter_movement: f64,
    pub stay_movement: f64,
    pub enter_confidence: f64,
    pub stay_confidence: f64,
    pub max_gap_ms: f64,
    pub new_page_movement: f64,
    pub min_sharpness: f64,
    pub min_brightness: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            steady_ms: STEADY_MS,
            enter_movement: ENTER_MOVEMENT,
            stay_movement: STAY_MOVEMENT,
            enter_confidence: ENTER_CONFIDENCE,
            stay_confidence: STAY_CONFIDENCE,
            max_gap_ms: MAX_GAP_MS,
            new_page_movement: NEW_PAGE_MOVEMENT,
            min_sharpness: MIN_SHARPNESS,
            min_brightness: MIN_BRIGHTNESS,
        }
    }
}

pub struct CaptureStability {
    options: Options,
    state: CaptureState,
    previous: Option<Quad>,
    anchor: Option<Quad>,
    captured: Option<Quad>,
    steady_since: f64,
    last_seen: f64,
    last_frame: f64,
    missing: bool,
    soft_since: Option<f64>,
}

impl Default for CaptureStability {
    fn default() -> Self {
        Self::new(Options::default())
    }
}

impl CaptureStability {
    pub fn new(options: Options) -> Self {
        CaptureStability {
            options,
            state: CaptureState::NoPage,
            previous: None,
            anchor: None,
            captured: None,
            steady_since: 0.0,
            last_seen: f64::NEG_INFINITY,
            last_frame: f64::NEG_INFINITY,
            missing: false,
            soft_since: None,
        }
    }

    pub fn state(&self) -> CaptureState {
        self.state
    }

    /// Forget everything (a new document, a photo taken by hand): the next steady page is taken.
    pub fn reset(&mut self) {
        self.state = CaptureState::NoPage;
        self.previous = None;
        self.anchor = None;
        self.captured = None;
        self.last_seen = f64::NEG_INFINITY;
        self.last_frame = f64::NEG_INFINITY;
        self.missing = false;
        self.soft_since = None;
    }

    /// Frames come again after a pause, the first of them taken at `timestamp_ms`: the time since the last frame counts
    /// neither as time the page was missing nor as time it held still.
    pub fn resume(&mut self, timestamp_ms: f64) {
        if self.last_frame == f64::NEG_INFINITY || timestamp_ms <= self.last_frame {
            return;
        }
        let paused = timestamp_ms - self.last_frame;
        self.last_frame = timestamp_ms;
        self.last_seen += paused;
        self.steady_since += paused;
        if let Some(s) = &mut self.soft_since {
            *s += paused;
        }
    }

    /// The page in view was taken another way (the shutter pressed by hand): it counts as captured.
    pub fn mark_captured(&mut self) {
        let Some(quad) = self.previous else { return };
        self.state = CaptureState::Captured;
        self.captured = Some(quad);
        self.anchor = None;
    }

    /// Dark below the minimum brightness, else blurry below the minimum sharpness, else (and when not measured) good.
    pub fn judge(
        sharpness: Option<f64>,
        brightness: Option<f64>,
        options: &Options,
    ) -> CaptureQuality {
        if brightness.is_some_and(|b| b < options.min_brightness) {
            return CaptureQuality::Dark;
        }
        if sharpness.is_some_and(|s| s < options.min_sharpness) {
            return CaptureQuality::Blurry;
        }
        CaptureQuality::Good
    }

    /// Cut off when a corner is within EDGE_MARGIN of the diagonal from the frame's edge, else far below MIN_AREA_SHARE of
    /// the frame's area.
    pub fn placement(quad: &Quad, frame_width: u32, frame_height: u32) -> CapturePlacement {
        let (w, h) = (frame_width as f64, frame_height as f64);
        let margin = EDGE_MARGIN * (w * w + h * h).sqrt();
        if !quad.is_inside(w, h, margin) {
            return CapturePlacement::CutOff;
        }
        if quad.signed_area().abs() < MIN_AREA_SHARE * w * h {
            return CapturePlacement::Far;
        }
        CapturePlacement::Fine
    }

    pub fn update(&mut self, frame: &Observation) -> Decision {
        let quality = Self::judge(frame.sharpness, frame.brightness, &self.options);
        self.last_frame = frame.timestamp_ms;
        let (state, should_capture, progress, suggests_shutter) = self.decide(frame, quality);
        Decision {
            state,
            should_capture,
            progress,
            quality,
            suggests_shutter,
        }
    }

    fn progress(&self, t: f64) -> f64 {
        ((t - self.steady_since) / self.options.steady_ms).clamp(0.0, 1.0)
    }

    fn decide(
        &mut self,
        frame: &Observation,
        quality: CaptureQuality,
    ) -> (CaptureState, bool, f64, bool) {
        let o = self.options;
        let (fw, fh) = (frame.frame_width as f64, frame.frame_height as f64);
        let diagonal = (fw * fw + fh * fh).sqrt();
        let t = frame.timestamp_ms;
        // only a held, blurred page keeps the time; a short gap in the frames does not break it
        let blurred_since = self.soft_since.take();

        let Some(quad) = frame.quad else {
            if t - self.last_seen <= o.max_gap_ms {
                self.soft_since = blurred_since;
                // waiting for the page to come back; the steady time stands still meanwhile
                self.missing = true;
                let progress = if self.state == CaptureState::Steadying {
                    self.progress(self.last_seen)
                } else {
                    0.0
                };
                return (self.state, false, progress, false);
            }
            self.previous = None;
            self.anchor = None;
            self.captured = None;
            self.missing = false;
            self.state = CaptureState::NoPage;
            return (self.state, false, 0.0, false);
        };

        let returning = t - self.last_seen > o.max_gap_ms;
        // back after frames without it: the time it was missing is not time held still
        if self.missing && !returning {
            self.steady_since += t - self.last_seen;
        }
        self.missing = false;
        self.last_seen = t;
        let previous = self.previous;
        self.previous = Some(quad);

        if self.state == CaptureState::Captured {
            // the page taken stays taken, however long since the last frame, until it moves away
            if let Some(taken) = self.captured
                && quad.movement(&taken) < o.new_page_movement * diagonal
            {
                return (self.state, false, 1.0, false);
            }
            self.captured = None;
            self.state = CaptureState::Moving;
        }
        let dips = |state: &mut CaptureState| {
            *state = if frame.confidence >= o.stay_confidence {
                CaptureState::Moving
            } else {
                CaptureState::NoPage
            };
        };
        if returning {
            self.anchor = None;
            dips(&mut self.state);
            return (self.state, false, 0.0, false);
        }

        if let (CaptureState::Steadying, Some(anchor)) = (self.state, self.anchor) {
            let stays = frame.confidence >= o.stay_confidence
                && quad.movement(&anchor) <= o.stay_movement * diagonal;
            if stays {
                if t - self.steady_since >= o.steady_ms {
                    // held still long enough: taken from the first frame that is sharp and lit
                    if quality != CaptureQuality::Good {
                        let mut suggests = false;
                        if quality == CaptureQuality::Blurry {
                            let since = blurred_since.unwrap_or(t);
                            self.soft_since = Some(since);
                            suggests = t - since >= SOFT_HINT_MS;
                        }
                        return (self.state, false, 1.0, suggests);
                    }
                    self.state = CaptureState::Captured;
                    self.captured = Some(quad);
                    self.anchor = None;
                    return (self.state, true, 1.0, false);
                }
                return (self.state, false, self.progress(t), false);
            }
            self.anchor = None;
            dips(&mut self.state);
            return (self.state, false, 0.0, false);
        }

        if frame.confidence < o.enter_confidence {
            dips(&mut self.state);
            return (self.state, false, 0.0, false);
        }
        if let Some(last) = previous
            && quad.movement(&last) <= o.enter_movement * diagonal
        {
            self.state = CaptureState::Steadying;
            self.anchor = Some(last);
            self.steady_since = t;
            return (self.state, false, 0.0, false);
        }
        self.state = CaptureState::Moving;
        (self.state, false, 0.0, false)
    }
}
