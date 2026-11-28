//! Colour spaces (ISO 32000-1 8.6) as a content stream selects them, reduced to what turning a colour into sRGB
//! needs. Conversions use no profiles: CMYK is (1-c)(1-k), Lab and the calibrated spaces are scaled to D65.

use std::sync::Arc;

use lazy_pdf_syntax::{Dict, Document, Name, Object};

use crate::function::{Function, FunctionError, clamp, numbers, ordered_pairs};

const MAX_DEPTH: usize = 8;

/// A colour as 8-bit sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// A colour space reduced to what turning samples into sRGB needs.
#[derive(Debug)]
pub(crate) enum Sample {
    Gray,
    Rgb,
    Cmyk,
    CalRgb {
        gamma: [f64; 3],
        matrix: [f64; 9],
        white: [f64; 3],
    },
    Lab {
        range: [f64; 4],
        white: [f64; 3],
    },
    Indexed {
        palette: Vec<u8>,
        high: usize,
    },
    Tint {
        components: usize,
        alternate: Arc<Sample>,
        function: Arc<Function>,
    },
}

type Res<T> = Result<T, FunctionError>;

fn byte(v: f64) -> u8 {
    let scaled = clamp(v, 0.0, 1.0) * 255.0 + 0.5;
    if scaled.is_nan() { 0 } else { scaled as u8 }
}

fn simple(name: &[u8]) -> Res<Sample> {
    match name {
        b"DeviceGray" | b"G" | b"CalGray" => Ok(Sample::Gray),
        b"DeviceRGB" | b"RGB" | b"CalRGB" => Ok(Sample::Rgb),
        b"DeviceCMYK" | b"CMYK" => Ok(Sample::Cmyk),
        _ => Err(FunctionError),
    }
}

fn resolved(doc: &mut Document, value: &Object) -> Option<Arc<Object>> {
    doc.resolve(value).map(|v| v.into_shared())
}

fn white_point(doc: &mut Document, d: &Dict) -> Option<[f64; 3]> {
    let white = numbers(doc, d.get("WhitePoint"))?;
    (white.len() == 3 && white[0] > 0.0 && white[1] > 0.0 && white[2] > 0.0)
        .then(|| [white[0], white[1], white[2]])
}

impl Sample {
    pub(crate) fn components(&self) -> usize {
        match self {
            Sample::Gray | Sample::Indexed { .. } => 1,
            Sample::Rgb | Sample::CalRgb { .. } | Sample::Lab { .. } => 3,
            Sample::Cmyk => 4,
            Sample::Tint { components, .. } => *components,
        }
    }

    fn min(&self, component: usize) -> f64 {
        match self {
            Sample::Lab { range, .. } if component > 0 => range[(component - 1) * 2],
            _ => 0.0,
        }
    }

    fn max(&self, component: usize) -> f64 {
        match self {
            Sample::Lab { range, .. } => {
                if component == 0 {
                    100.0
                } else {
                    range[(component - 1) * 2 + 1]
                }
            }
            Sample::Indexed { high, .. } => *high as f64,
            _ => 1.0,
        }
    }

    pub(crate) fn parse(doc: &mut Document, space: &Object, depth: usize) -> Res<Sample> {
        if depth > MAX_DEPTH {
            return Err(FunctionError);
        }
        let space = resolved(doc, space).ok_or(FunctionError)?;
        if let Object::Name(name) = &*space {
            return simple(name.as_bytes());
        }
        let array = space
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or(FunctionError)?;
        let family = resolved(doc, &array[0])
            .and_then(|f| f.as_name().cloned())
            .ok_or(FunctionError)?;
        let item = |doc: &mut Document, i: usize| array.get(i).and_then(|v| resolved(doc, v));
        match family.as_bytes() {
            b"ICCBased" => {
                let profile = item(doc, 1).ok_or(FunctionError)?;
                let Object::Stream(profile) = &*profile else {
                    return Err(FunctionError);
                };
                let n = profile
                    .dict
                    .get("N")
                    .and_then(|v| resolved(doc, v))
                    .and_then(|v| v.as_number().and_then(|n| n.whole()))
                    .unwrap_or(0);
                match n {
                    1 => return Ok(Sample::Gray),
                    3 => return Ok(Sample::Rgb),
                    4 => return Ok(Sample::Cmyk),
                    _ => {}
                }
                match profile.dict.get("Alternate") {
                    Some(alternate) => Self::parse(doc, alternate, depth + 1),
                    None => Err(FunctionError),
                }
            }
            b"CalGray" => Ok(Sample::Gray),
            b"CalRGB" => {
                let mut gamma = [1.0; 3];
                let mut matrix = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
                let mut white = [0.9505, 1.0, 1.089];
                if let Some(obj) = item(doc, 1)
                    && let Some(d) = obj.as_dict()
                {
                    if let Some(g) = numbers(doc, d.get("Gamma")).filter(|g| g.len() == 3) {
                        gamma.copy_from_slice(&g);
                    }
                    if let Some(m) = numbers(doc, d.get("Matrix")).filter(|m| m.len() == 9) {
                        matrix.copy_from_slice(&m);
                    }
                    if let Some(w) = white_point(doc, d) {
                        white = w;
                    }
                }
                Ok(Sample::CalRgb {
                    gamma,
                    matrix,
                    white,
                })
            }
            b"Lab" => {
                let mut range = [-100.0, 100.0, -100.0, 100.0];
                let mut white = [0.9642, 1.0, 0.8249];
                if let Some(obj) = item(doc, 1)
                    && let Some(d) = obj.as_dict()
                {
                    if let Some(r) =
                        numbers(doc, d.get("Range")).filter(|r| r.len() == 4 && ordered_pairs(r))
                    {
                        range.copy_from_slice(&r);
                    }
                    if let Some(w) = white_point(doc, d) {
                        white = w;
                    }
                }
                Ok(Sample::Lab { range, white })
            }
            b"Indexed" | b"I" => {
                let base = array.get(1).ok_or(FunctionError)?;
                let high = item(doc, 2)
                    .and_then(|v| v.as_number().and_then(|n| n.whole()))
                    .unwrap_or(0);
                let lookup = item(doc, 3);
                Self::indexed(doc, base, high, lookup.as_deref(), depth)
            }
            b"Separation" | b"DeviceN" => {
                let count = if family.as_bytes() == b"Separation" {
                    1
                } else {
                    item(doc, 1)
                        .and_then(|v| v.as_array().map(<[Object]>::len))
                        .unwrap_or(0)
                };
                if !(1..=32).contains(&count) {
                    return Err(FunctionError);
                }
                let alternate = Self::parse(doc, array.get(2).ok_or(FunctionError)?, depth + 1)?;
                let function = Function::parse(doc, array.get(3).ok_or(FunctionError)?)?;
                if function.inputs != count || function.outputs < alternate.components() {
                    return Err(FunctionError);
                }
                Ok(Sample::Tint {
                    components: count,
                    alternate: Arc::new(alternate),
                    function,
                })
            }
            _ => {
                if array.len() == 1 {
                    simple(family.as_bytes())
                } else {
                    Err(FunctionError)
                }
            }
        }
    }

    fn indexed(
        doc: &mut Document,
        base: &Object,
        high: i64,
        lookup: Option<&Object>,
        depth: usize,
    ) -> Res<Sample> {
        let space = Self::parse(doc, base, depth + 1)?;
        if matches!(space, Sample::Indexed { .. }) {
            return Err(FunctionError);
        }
        let high = high.clamp(0, 255) as usize;
        let table: Vec<u8> = match lookup {
            Some(Object::String(s)) => s.bytes.clone(),
            Some(Object::Stream(s)) => doc.stream_data(s).map_err(|_| FunctionError)?,
            _ => return Err(FunctionError),
        };
        let n = space.components();
        let mut palette = vec![0u8; (high + 1) * 3];
        let mut comps = vec![0.0; n];
        for i in 0..=high {
            for (j, slot) in comps.iter_mut().enumerate() {
                let unit = table.get(i * n + j).map_or(0.0, |&b| b as f64 / 255.0);
                *slot = space.min(j) + unit * (space.max(j) - space.min(j));
            }
            palette[i * 3..i * 3 + 3].copy_from_slice(&space.to_rgb(&comps)?);
        }
        Ok(Sample::Indexed { palette, high })
    }

    /// `c` holds at least `components` values.
    pub(crate) fn to_rgb(&self, c: &[f64]) -> Res<[u8; 3]> {
        Ok(match self {
            Sample::Gray => {
                let v = byte(c[0]);
                [v, v, v]
            }
            Sample::Rgb => [byte(c[0]), byte(c[1]), byte(c[2])],
            Sample::Cmyk => {
                let k = 1.0 - clamp(c[3], 0.0, 1.0);
                let channel = |v: f64| byte((1.0 - clamp(v, 0.0, 1.0)) * k);
                [channel(c[0]), channel(c[1]), channel(c[2])]
            }
            Sample::CalRgb {
                gamma,
                matrix,
                white,
            } => {
                let a = clamp(c[0], 0.0, 1.0).powf(gamma[0]);
                let b = clamp(c[1], 0.0, 1.0).powf(gamma[1]);
                let g = clamp(c[2], 0.0, 1.0).powf(gamma[2]);
                srgb(
                    matrix[0] * a + matrix[3] * b + matrix[6] * g,
                    matrix[1] * a + matrix[4] * b + matrix[7] * g,
                    matrix[2] * a + matrix[5] * b + matrix[8] * g,
                    white,
                )
            }
            Sample::Lab { range, white } => {
                let m = (clamp(c[0], 0.0, 100.0) + 16.0) / 116.0;
                let l = m + clamp(c[1], range[0], range[1]) / 500.0;
                let n = m - clamp(c[2], range[2], range[3]) / 200.0;
                srgb(
                    white[0] * lab_inverse(l),
                    white[1] * lab_inverse(m),
                    white[2] * lab_inverse(n),
                    white,
                )
            }
            Sample::Indexed { palette, high } => {
                let index = ((c[0] + 0.5) as i32).clamp(0, *high as i32) as usize;
                [
                    palette[index * 3],
                    palette[index * 3 + 1],
                    palette[index * 3 + 2],
                ]
            }
            Sample::Tint {
                alternate,
                function,
                ..
            } => {
                let mut values = vec![0.0; function.outputs];
                function.evaluate(c, &mut values)?;
                for (i, v) in values.iter_mut().enumerate().take(alternate.components()) {
                    *v = clamp(*v, alternate.min(i), alternate.max(i));
                }
                alternate.to_rgb(&values)?
            }
        })
    }
}

fn lab_inverse(x: f64) -> f64 {
    if x >= 6.0 / 29.0 {
        x * x * x
    } else {
        108.0 / 841.0 * (x - 4.0 / 29.0)
    }
}

// XYZ relative to `white` to sRGB: the white is scaled to D65, crude next to a proper adaptation and enough for a
// preview.
fn srgb(x: f64, y: f64, z: f64, white: &[f64; 3]) -> [u8; 3] {
    let x = x * (0.9505 / white[0]);
    let y = y / white[1];
    let z = z * (1.089 / white[2]);
    [
        byte(encode(3.2406 * x - 1.5372 * y - 0.4986 * z)),
        byte(encode(-0.9689 * x + 1.8758 * y + 0.0415 * z)),
        byte(encode(0.0557 * x - 0.2040 * y + 1.0570 * z)),
    ]
}

fn encode(linear: f64) -> f64 {
    let linear = clamp(linear, 0.0, 1.0);
    if linear <= 0.0031308 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// A colour space as a content stream selects it.
#[derive(Debug)]
pub struct ColorSpaceInfo {
    /// DeviceGray, DeviceRGB, DeviceCMYK, CalGray, CalRGB, Lab, ICCBased, Indexed, Separation, DeviceN or Pattern.
    pub family: String,
    /// Numbers a colour of this space has; for Pattern those of the underlying space (0 when it has none).
    pub components: usize,
    /// The underlying space of an uncoloured pattern space.
    pub base: Option<Arc<ColorSpaceInfo>>,
    converter: Option<Arc<Sample>>,
}

impl ColorSpaceInfo {
    pub fn device_gray() -> Arc<ColorSpaceInfo> {
        Self::device("DeviceGray", 1, Sample::Gray)
    }

    pub fn device_rgb() -> Arc<ColorSpaceInfo> {
        Self::device("DeviceRGB", 3, Sample::Rgb)
    }

    pub fn device_cmyk() -> Arc<ColorSpaceInfo> {
        Self::device("DeviceCMYK", 4, Sample::Cmyk)
    }

    fn device(family: &str, components: usize, sample: Sample) -> Arc<ColorSpaceInfo> {
        Arc::new(ColorSpaceInfo {
            family: family.into(),
            components,
            base: None,
            converter: Some(Arc::new(sample)),
        })
    }

    /// The space a name or array in the file stands for; device gray for anything that does not read.
    pub fn create(doc: &mut Document, value: &Object, depth: usize) -> Arc<ColorSpaceInfo> {
        let Some(value) = resolved(doc, value) else {
            return Self::device_gray();
        };
        if depth > MAX_DEPTH {
            return Self::device_gray();
        }
        if let Object::Name(name) = &*value {
            return match name.as_bytes() {
                b"DeviceRGB" | b"RGB" | b"CalRGB" => Self::device_rgb(),
                b"DeviceCMYK" | b"CMYK" => Self::device_cmyk(),
                b"Pattern" => Arc::new(ColorSpaceInfo {
                    family: "Pattern".into(),
                    components: 0,
                    base: None,
                    converter: None,
                }),
                _ => Self::device_gray(),
            };
        }
        let Some(array) = value.as_array().filter(|a| !a.is_empty()) else {
            return Self::device_gray();
        };
        let Some(family) = resolved(doc, &array[0]).and_then(|f| f.as_name().cloned()) else {
            return Self::device_gray();
        };
        let family = family.text();
        if family == "Pattern" {
            let under = array.get(1).map(|u| Self::create(doc, u, depth + 1));
            return Arc::new(ColorSpaceInfo {
                family: "Pattern".into(),
                components: under.as_ref().map_or(0, |u| u.components),
                base: under,
                converter: None,
            });
        }
        let components = match family.as_str() {
            "DeviceGray" | "G" | "CalGray" | "Indexed" | "I" | "Separation" => 1,
            "DeviceRGB" | "RGB" | "CalRGB" | "Lab" => 3,
            "DeviceCMYK" | "CMYK" => 4,
            "ICCBased" => {
                let n = array
                    .get(1)
                    .and_then(|v| resolved(doc, v))
                    .and_then(|v| match &*v {
                        Object::Stream(s) => s.dict.get("N").cloned(),
                        _ => None,
                    })
                    .and_then(|n| resolved(doc, &n))
                    .and_then(|n| n.as_number().and_then(|n| n.whole()));
                match n {
                    Some(n @ (1 | 3 | 4)) => n as usize,
                    _ => 3,
                }
            }
            "DeviceN" => array
                .get(1)
                .and_then(|v| resolved(doc, v))
                .and_then(|v| v.as_array().map(|a| a.len().clamp(1, 32)))
                .unwrap_or(1),
            _ => 1,
        };
        let family = match family.as_str() {
            "G" => "DeviceGray",
            "RGB" => "DeviceRGB",
            "CMYK" => "DeviceCMYK",
            "I" => "Indexed",
            other => other,
        };
        let converter = Sample::parse(doc, &value, 0).ok().map(Arc::new);
        Arc::new(ColorSpaceInfo {
            family: family.into(),
            components,
            base: None,
            converter,
        })
    }

    /// What a colour starts as in this space (8.6.5.2).
    pub fn initial_color(&self) -> Vec<f64> {
        match self.family.as_str() {
            "DeviceCMYK" => vec![0.0, 0.0, 0.0, 1.0],
            "Separation" | "DeviceN" => vec![1.0; self.components],
            _ => vec![0.0; self.components],
        }
    }

    pub fn to_rgb(&self, c: &[f64]) -> RgbColor {
        let mut padded = [0.0; 32];
        for (slot, v) in padded.iter_mut().zip(c) {
            *slot = *v;
        }
        let fallback = match self.components {
            3 => Sample::Rgb,
            4 => Sample::Cmyk,
            _ => Sample::Gray,
        };
        let space = self.converter.as_deref().unwrap_or(&fallback);
        match space.to_rgb(&padded) {
            Ok([r, g, b]) => RgbColor { r, g, b },
            Err(_) => RgbColor { r: 0, g: 0, b: 0 },
        }
    }
}

/// A colour set by a colour operator: its space, its numbers, and for a pattern space the pattern (a tiling
/// pattern's stream or a shading pattern's dictionary, resolved).
#[derive(Debug, Clone)]
pub struct PdfColor {
    pub space: Arc<ColorSpaceInfo>,
    pub components: Vec<f64>,
    pub pattern_name: Option<Name>,
    pub pattern: Option<Arc<Object>>,
}

impl PdfColor {
    pub fn new(space: Arc<ColorSpaceInfo>, components: Vec<f64>) -> PdfColor {
        PdfColor {
            space,
            components,
            pattern_name: None,
            pattern: None,
        }
    }

    pub fn black() -> PdfColor {
        PdfColor::new(ColorSpaceInfo::device_gray(), vec![0.0])
    }

    /// The colour as sRGB; for a pattern the underlying colour of an uncoloured pattern, else black.
    pub fn rgb(&self) -> RgbColor {
        if self.space.family == "Pattern" {
            return match &self.space.base {
                Some(under) if self.components.len() >= under.components => {
                    under.to_rgb(&self.components)
                }
                _ => RgbColor { r: 0, g: 0, b: 0 },
            };
        }
        self.space.to_rgb(&self.components)
    }
}
