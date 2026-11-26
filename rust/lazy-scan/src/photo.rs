use crate::corners::SURE_CONFIDENCE;
use crate::detector::{Detection, Options as DetectorOptions, Rejection, detect};
use crate::enhancer::{DEFAULT_THRESHOLD, EnhanceMode, enhance};
use crate::geometry::{Point, Quad};
use crate::pipeline::locate;
use crate::refiner::refine;
use crate::warp::{self, DOCUMENTS, PAPER_ASPECT_TOLERANCE};
use lazy_image::Rgba;
use lazy_scan_model::CornerModel;
use std::borrow::Cow;

// A photo to a finished page, as Lazy Scan's PhotoImport and Lazy Send's page pipeline do it: turned upright by its
// orientation tag, the page looked for (the corner model first), cut out along its corners with the perspective corrected,
// then cleaned up with the chosen filter. The two apps size the page differently; `PageSizing` picks the rules.

/// The resolution of a page whose paper size is not known.
pub const NOMINAL_DPI: u32 = 200;
/// The lowest resolution a page from a photo is given, and the highest.
pub const MIN_DPI: u32 = 50;
pub const MAX_DPI: u32 = 1200;

/// Where Letter is the usual paper; A4 everywhere else.
const LETTER_REGIONS: [&str; 14] = [
    "US", "CA", "MX", "PH", "CL", "CO", "CR", "DO", "GT", "NI", "PA", "PR", "SV", "VE",
];

/// How a photo's stored pixels are turned to show it the right way up: mirrored left to right first when `mirrored`, then
/// turned `quarter_turns` quarter turns clockwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Orientation {
    pub quarter_turns: i32,
    pub mirrored: bool,
}

impl Orientation {
    pub const UPRIGHT: Orientation = Orientation {
        quarter_turns: 0,
        mirrored: false,
    };

    /// The turn for the EXIF and TIFF tag 274: 1 upright, 2 mirrored, 3 upside down, 4 mirrored top to bottom, 5 mirrored
    /// across the main diagonal, 6 a quarter turn clockwise, 7 mirrored across the other diagonal, 8 a quarter turn
    /// counterclockwise. Any other value (no tag, 0, 9 and up) is upright.
    pub fn from_exif(value: u32) -> Orientation {
        let (quarter_turns, mirrored) = match value {
            2 => (0, true),
            3 => (2, false),
            4 => (2, true),
            5 => (3, true),
            6 => (1, false),
            7 => (1, true),
            8 => (3, false),
            _ => (0, false),
        };
        Orientation {
            quarter_turns,
            mirrored,
        }
    }

    pub fn is_upright(&self) -> bool {
        self.quarter_turns.rem_euclid(4) == 0 && !self.mirrored
    }
}

/// The photo the right way up; borrowed as it is when it already is.
pub fn oriented(stored: &Rgba, orientation: Orientation) -> Cow<'_, Rgba> {
    let turns = orientation.quarter_turns.rem_euclid(4);
    match (orientation.mirrored, turns) {
        (false, 0) => Cow::Borrowed(stored),
        (true, 0) => Cow::Owned(stored.mirrored()),
        (false, _) => Cow::Owned(stored.rotated_quarter_turns(orientation.quarter_turns)),
        (true, _) => Cow::Owned(
            stored
                .mirrored()
                .rotated_quarter_turns(orientation.quarter_turns),
        ),
    }
}

/// The clean-up a page gets: Original leaves the pixels as cut out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Original,
    Enhanced,
    Grayscale,
    BlackAndWhite,
    Whiteboard,
}

impl Filter {
    pub fn mode(self) -> Option<EnhanceMode> {
        match self {
            Filter::Original => None,
            Filter::Enhanced => Some(EnhanceMode::Auto),
            Filter::Grayscale => Some(EnhanceMode::Gray),
            Filter::BlackAndWhite => Some(EnhanceMode::BlackAndWhite),
            Filter::Whiteboard => Some(EnhanceMode::Whiteboard),
        }
    }
}

/// Whether the page is cut out without asking, or the corner editor shows first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Correct,
    Review,
}

/// Why the corners are to be checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    None,
    /// An outline was found, but not clearly enough to trust.
    Unsure,
    /// The page runs off the edge of the photo.
    CutOff,
    /// Two pages of about the same size are in the photo.
    TwoPages,
    NothingFound,
}

/// What to do with one photo: cut the page out along `corners`, or show the corner editor from them with `reason` as its note.
/// `corners` are in the upright photo's pixels: the page found, else the best outline seen kept within the photo, else a
/// rectangle inset from the photo's edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImportPlan {
    pub action: Action,
    pub corners: Quad,
    pub reason: Reason,
    /// The corner model's confidence for a page it found, else the page search's.
    pub confidence: f64,
    pub by_model: bool,
}

/// The page the corner model finds in an upright photo, moved onto the photo's edges: cut out without asking when the model
/// and the edges are both sure and the page can be corrected, else to be reviewed as unsure. `None` when it finds no page or
/// one that is not a document's. An error comes from the model.
pub fn model_plan(
    photo: &Rgba,
    model: &dyn CornerModel,
) -> Result<Option<ImportPlan>, lazy_scan_model::ModelError> {
    Ok(locate(photo, model)?.map(|found| ImportPlan {
        action: if found.sure {
            Action::Correct
        } else {
            Action::Review
        },
        corners: found.quad,
        reason: if found.sure {
            Reason::None
        } else {
            Reason::Unsure
        },
        confidence: found.confidence,
        by_model: true,
    }))
}

/// How a page's resolution is worked out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpiRule {
    /// Lazy Scan's: a cut page takes the resolution of the paper size it matches, else `NOMINAL_DPI`; a whole photo takes the
    /// paper its shape matches, else the usual paper's length in the user's region.
    Scan,
    /// Lazy Send's: a cut page takes the resolution of the paper size it matches, else its long side over the usual paper's
    /// length in the user's region; a whole photo always does the latter, without matching its shape.
    Send,
}

/// The size rules of a page made from a photo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSizing {
    /// The longest side of a cut page in pixels; 0 for no limit. A whole photo is never scaled.
    pub max_long_side: usize,
    pub rule: DpiRule,
}

impl PageSizing {
    /// Lazy Scan's PhotoImport: no cap.
    pub const SCAN: PageSizing = PageSizing {
        max_long_side: 0,
        rule: DpiRule::Scan,
    };
    /// Lazy Send's PagePipeline: a cut page at most 3508 px long (A4 at 300 dpi), however close the camera was.
    pub const SEND: PageSizing = PageSizing {
        max_long_side: 3508,
        rule: DpiRule::Send,
    };
}

/// A page cut out of its photo.
#[derive(Clone, Debug)]
pub struct Page {
    pub image: Rgba,
    pub dpi: u32,
    /// The clean-up applied, `Auto` resolved to colour or gray; `None` for Original.
    pub mode: Option<EnhanceMode>,
    /// The corners it was cut along, in the upright photo.
    pub corners: Quad,
    pub whole_photo: bool,
}

/// `dpi` rounded and kept within `MIN_DPI` to `MAX_DPI`; `NOMINAL_DPI` when the size is not known.
pub fn page_dpi(dpi: Option<f64>) -> u32 {
    match dpi {
        Some(d) if d.is_finite() && d > 0.0 => {
            ((d + 0.5).floor() as i64).clamp(MIN_DPI as i64, MAX_DPI as i64) as u32
        }
        _ => NOMINAL_DPI,
    }
}

fn usual_paper_long_inches(region: Option<&str>, a4: f64) -> f64 {
    let code = region.unwrap_or("").trim().to_uppercase();
    if LETTER_REGIONS.contains(&code.as_str()) {
        11.0
    } else {
        a4
    }
}

/// Lazy Send's `pageDPI`: `dpi` rounded and kept within `MIN_DPI` to `MAX_DPI`; when it is not known, the resolution that
/// makes `long_side` the long side of the usual paper where the user lives (11 in for Letter, 11.69 in for A4).
pub fn page_dpi_by_long_side(dpi: Option<f64>, long_side: usize, region: Option<&str>) -> u32 {
    match dpi {
        Some(d) if d.is_finite() && d > 0.0 => page_dpi(Some(d)),
        _ if long_side > 0 => page_dpi(Some(
            long_side as f64 / usual_paper_long_inches(region, 11.69),
        )),
        _ => page_dpi(None),
    }
}

/// The resolution of a whole photo taken as the page: from a paper size its shape matches, else the one that makes its long
/// side the usual paper's where the user lives (`region` is a two-letter country code).
pub fn whole_photo_dpi(width: usize, height: usize, region: Option<&str>) -> u32 {
    if width == 0 || height == 0 {
        return NOMINAL_DPI;
    }
    let long = width.max(height) as f64;
    let short = width.min(height) as f64;
    let inches = match warp::match_paper(long / short, PAPER_ASPECT_TOLERANCE, &DOCUMENTS) {
        Some(m) => {
            let (w, h) = m.paper.inches();
            w.max(h)
        }
        None => usual_paper_long_inches(region, 11.6929),
    };
    page_dpi(Some(long / inches))
}

fn cleaned(image: Rgba, filter: Filter) -> (Rgba, Option<EnhanceMode>) {
    match filter.mode() {
        Some(mode) => {
            let done = enhance(&image, mode, DEFAULT_THRESHOLD);
            (done.image, Some(done.mode))
        }
        None => (image, None),
    }
}

/// The page cut out of `photo` along `corners` (named again from the top left), its perspective corrected, then cleaned up.
/// `None` when the corners outline no page.
pub fn corrected_page(
    photo: &Rgba,
    corners: &Quad,
    filter: Filter,
    sizing: PageSizing,
    region: Option<&str>,
) -> Option<Page> {
    let named = Quad::from_corners(corners.0);
    let options = warp::Options {
        max_long_side: sizing.max_long_side,
        ..warp::Options::default()
    };
    let (cut, plan) = warp::correct(photo, &named, &options)?;
    let dpi = match sizing.rule {
        DpiRule::Scan => page_dpi(plan.dpi),
        DpiRule::Send => page_dpi_by_long_side(plan.dpi, cut.width.max(cut.height), region),
    };
    let (image, mode) = cleaned(cut, filter);
    Some(Page {
        image,
        dpi,
        mode,
        corners: named,
        whole_photo: false,
    })
}

/// The whole photo as the page, with only the filter applied. `None` for a photo without pixels.
pub fn whole_photo_page(
    photo: &Rgba,
    filter: Filter,
    sizing: PageSizing,
    region: Option<&str>,
) -> Option<Page> {
    if photo.width == 0 || photo.height == 0 {
        return None;
    }
    let dpi = match sizing.rule {
        DpiRule::Scan => whole_photo_dpi(photo.width, photo.height, region),
        DpiRule::Send => page_dpi_by_long_side(None, photo.width.max(photo.height), region),
    };
    let (image, mode) = cleaned(photo.clone(), filter);
    Some(Page {
        image,
        dpi,
        mode,
        corners: Quad::from_xy([
            [0.0, 0.0],
            [photo.width as f64, 0.0],
            [photo.width as f64, photo.height as f64],
            [0.0, photo.height as f64],
        ]),
        whole_photo: true,
    })
}

/// What a photo became: the plan, the page made by it (a review is cut from the corners the editor would start from, so a
/// caller that does not ask still has a page; the whole photo when nothing can be cut), and whether the corner model failed
/// to run, in which case the page search stood in as the apps do.
#[derive(Clone, Debug)]
pub struct Imported {
    pub plan: ImportPlan,
    pub page: Page,
    pub model_failed: bool,
}

/// `quad` with every corner moved into the photo; the start rectangle when that leaves no usable outline.
pub fn clamped_to_photo(quad: &Quad, width: f64, height: f64) -> Quad {
    if !(width >= 0.0 && height >= 0.0 && width.is_finite() && height.is_finite()) {
        return *quad;
    }
    let clamped = quad.clamped_to_photo(width, height);
    if clamped.usable() {
        clamped
    } else {
        Quad::start(width, height)
    }
}

/// `quad` with corner `index` (0 top left, then clockwise, wrapping) moved to `to`, kept within the photo; the quad as it was
/// when the move would leave corners the page cannot be cut out along.
pub fn moved_corner(quad: &Quad, index: usize, to: Point, width: f64, height: f64) -> Quad {
    if !(width >= 0.0 && height >= 0.0 && width.is_finite() && height.is_finite()) {
        return *quad;
    }
    let mut corners = quad.0;
    corners[index & 3] = Point::new(to.x.clamp(0.0, width), to.y.clamp(0.0, height));
    let moved = Quad(corners);
    if moved.usable() { moved } else { *quad }
}

/// What to do with a photo of `width` x `height` where the page search found `detection`: a page found with at least
/// SURE_CONFIDENCE (and that can be corrected) is cut out; anything else is shown in the corner editor, starting from the page
/// found, else the best outline seen (kept within the photo), else a rectangle inset from the photo's edges.
pub fn plan_detection(detection: &Detection, width: usize, height: usize) -> ImportPlan {
    let (w, h) = (width as f64, height as f64);
    if let Some(quad) = detection.quad {
        let sure = detection.confidence >= SURE_CONFIDENCE && quad.correctable(w, h);
        return ImportPlan {
            action: if sure {
                Action::Correct
            } else {
                Action::Review
            },
            corners: if sure {
                quad
            } else {
                clamped_to_photo(&quad, w, h)
            },
            reason: if sure { Reason::None } else { Reason::Unsure },
            confidence: detection.confidence,
            by_model: false,
        };
    }
    let reason = match detection.rejection {
        Rejection::CutOff => Reason::CutOff,
        Rejection::Ambiguous => Reason::TwoPages,
        Rejection::LowConfidence => Reason::Unsure,
        Rejection::None | Rejection::NothingFound => Reason::NothingFound,
    };
    ImportPlan {
        action: Action::Review,
        corners: detection
            .candidate
            .map_or_else(|| Quad::start(w, h), |c| clamped_to_photo(&c, w, h)),
        reason,
        confidence: detection.confidence,
        by_model: false,
    }
}

/// The page search for a photo the model did not place: the edge-based search, its corners moved onto the photo's edges.
fn search_plan(photo: &Rgba) -> ImportPlan {
    let mut detection = detect(photo, None, &DetectorOptions::default());
    if let Some(quad) = detection.quad {
        detection.quad = Some(refine(photo, &quad).quad);
    }
    plan_detection(&detection, photo.width, photo.height)
}

/// What to do with `photo` (upright), the corner model first when there is one.
pub fn plan(photo: &Rgba, model: Option<&dyn CornerModel>) -> (ImportPlan, bool) {
    let mut failed = false;
    if let Some(model) = model {
        match model_plan(photo, model) {
            Ok(Some(found)) => return (found, false),
            Ok(None) => {}
            Err(_) => failed = true,
        }
    }
    (search_plan(photo), failed)
}

/// One call from a photo as stored to a finished page: turned upright by `orientation`, planned, cut out and cleaned up with
/// `filter`. `region` is the user's two-letter country code, which sizes a page that matches no paper. `None` for a photo
/// without pixels.
pub fn import_photo(
    stored: &Rgba,
    orientation: Orientation,
    model: Option<&dyn CornerModel>,
    filter: Filter,
    sizing: PageSizing,
    region: Option<&str>,
) -> Option<Imported> {
    let photo = oriented(stored, orientation);
    if photo.width == 0 || photo.height == 0 {
        return None;
    }
    let (plan, model_failed) = plan(&photo, model);
    let page = corrected_page(&photo, &plan.corners, filter, sizing, region)
        .or_else(|| whole_photo_page(&photo, filter, sizing, region))?;
    Some(Imported {
        plan,
        page,
        model_failed,
    })
}
