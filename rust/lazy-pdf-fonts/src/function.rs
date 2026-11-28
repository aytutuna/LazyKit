//! PDF functions (ISO 32000-1 7.10): sampled (type 0), exponential (2), stitching (3) and PostScript calculator (4).
//! Inputs and outputs are clipped to /Domain and /Range.

use std::collections::HashMap;
use std::sync::Arc;

use lazy_pdf_syntax::{Document, Object, Stream};

const MAX_DEPTH: usize = 8;
const MAX_OUTPUTS: usize = 64;
const MAX_ARRAY_LENGTH: usize = 2_147_483_591;
const STACK_SIZE: usize = 100;
const MAX_NESTING: usize = 32;

/// A function that cannot be read or evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionError;

type Res<T> = Result<T, FunctionError>;

#[derive(Debug)]
pub struct Function {
    pub inputs: usize,
    pub outputs: usize,
    domain: Vec<f64>,
    range: Option<Vec<f64>>,
    kind: Kind,
}

#[derive(Debug)]
enum Kind {
    Combined(Vec<Arc<Function>>),
    Exponential {
        c0: Vec<f64>,
        c1: Vec<f64>,
        n: f64,
    },
    Stitching {
        functions: Vec<Arc<Function>>,
        bounds: Vec<f64>,
        encode: Vec<f64>,
    },
    Sampled {
        data: Vec<u8>,
        size: Vec<usize>,
        stride: Vec<usize>,
        bits: usize,
        encode: Vec<f64>,
        decode: Vec<f64>,
    },
    PostScript(Vec<Op>),
}

// Math.Clamp: a NaN passes through.
pub(crate) fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

fn interpolate(x: f64, x0: f64, x1: f64, y0: f64, y1: f64) -> f64 {
    if x1 == x0 {
        y0
    } else {
        y0 + (x - x0) * (y1 - y0) / (x1 - x0)
    }
}

fn ordered(pairs: &[f64]) -> bool {
    pairs.as_chunks::<2>().0.iter().all(|p| p[0] <= p[1])
}

fn resolved(doc: &mut Document, value: &Object) -> Option<Arc<Object>> {
    doc.resolve(value).map(|v| v.into_shared())
}

/// An array of numbers, resolved; none when it is not one or holds anything else.
pub(crate) fn numbers(doc: &mut Document, value: Option<&Object>) -> Option<Vec<f64>> {
    let array = resolved(doc, value?)?;
    let mut out = Vec::new();
    for item in array.as_array()? {
        out.push(resolved(doc, item)?.as_number()?.value());
    }
    Some(out)
}

pub(crate) fn ordered_pairs(values: &[f64]) -> bool {
    ordered(values)
}

fn int_of(doc: &mut Document, value: Option<&Object>, default: i64) -> i64 {
    value
        .and_then(|v| resolved(doc, v))
        .and_then(|v| v.as_number().and_then(|n| n.whole()))
        .unwrap_or(default)
}

type Parsed = HashMap<u32, Arc<Function>>;

impl Function {
    /// Reads a function dictionary or stream, or an array of one-output functions treated as one.
    pub fn parse(doc: &mut Document, function: &Object) -> Res<Arc<Function>> {
        let mut parsed = Parsed::new();
        Self::parse_at(doc, function, 0, &mut parsed)
    }

    // A function that several others share is parsed once; otherwise a few small objects name each other often
    // enough to take for ever.
    fn parse_at(
        doc: &mut Document,
        function: &Object,
        depth: usize,
        parsed: &mut Parsed,
    ) -> Res<Arc<Function>> {
        if depth > MAX_DEPTH {
            return Err(FunctionError);
        }
        let key = match function {
            Object::Ref(r) => Some(r.number),
            _ => None,
        };
        if let Some(known) = key.and_then(|k| parsed.get(&k)) {
            return Ok(known.clone());
        }
        let value = resolved(doc, function).ok_or(FunctionError)?;
        let result = Arc::new(Self::parse_one(doc, &value, depth, parsed)?);
        if result.outputs > MAX_OUTPUTS {
            return Err(FunctionError);
        }
        if let Some(k) = key {
            parsed.insert(k, result.clone());
        }
        Ok(result)
    }

    fn parse_one(
        doc: &mut Document,
        function: &Object,
        depth: usize,
        parsed: &mut Parsed,
    ) -> Res<Function> {
        if let Object::Array(items) = function {
            if items.is_empty() || items.len() > 64 {
                return Err(FunctionError);
            }
            let mut parts = Vec::new();
            for item in items {
                parts.push(Self::parse_at(doc, item, depth + 1, parsed)?);
            }
            let outputs = parts.iter().map(|p| p.outputs).sum();
            return Ok(Function {
                inputs: parts[0].inputs,
                outputs,
                domain: parts[0].domain.clone(),
                range: None,
                kind: Kind::Combined(parts),
            });
        }
        let (dict, stream) = match function {
            Object::Stream(s) => (&s.dict, Some(s)),
            Object::Dict(d) => (d, None),
            _ => return Err(FunctionError),
        };
        let domain = numbers(doc, dict.get("Domain")).ok_or(FunctionError)?;
        if domain.len() < 2 || domain.len() % 2 != 0 || domain.len() > 64 || !ordered(&domain) {
            return Err(FunctionError);
        }
        let mut range = numbers(doc, dict.get("Range"));
        if range.as_ref().is_some_and(|r| r.len() % 2 != 0) {
            range = None;
        }
        if let Some(r) = &range
            && (r.len() > 2 * MAX_OUTPUTS || !ordered(r))
        {
            return Err(FunctionError);
        }
        let kind = int_of(doc, dict.get("FunctionType"), -1);
        let mut made = match kind {
            0 => Self::sampled(doc, dict, stream, &domain, range.as_deref())?,
            2 => Self::exponential(doc, dict)?,
            3 => Self::stitching(doc, dict, depth, parsed)?,
            4 => Self::post_script(doc, stream, &domain, range.as_deref())?,
            _ => return Err(FunctionError),
        };
        made.domain = domain;
        made.range = range.filter(|r| r.len() >= made.outputs * 2);
        Ok(made)
    }

    fn exponential(doc: &mut Document, d: &lazy_pdf_syntax::Dict) -> Res<Function> {
        let c0 = numbers(doc, d.get("C0")).unwrap_or_else(|| vec![0.0]);
        let c1 = numbers(doc, d.get("C1")).unwrap_or_else(|| vec![1.0]);
        if c0.len() != c1.len() || c0.len() > 64 {
            return Err(FunctionError);
        }
        let n = d
            .get("N")
            .and_then(|v| resolved(doc, v))
            .and_then(|v| v.as_number().map(|n| n.value()))
            .unwrap_or(1.0);
        Ok(Function {
            inputs: 1,
            outputs: c0.len(),
            domain: vec![],
            range: None,
            kind: Kind::Exponential { c0, c1, n },
        })
    }

    fn stitching(
        doc: &mut Document,
        d: &lazy_pdf_syntax::Dict,
        depth: usize,
        parsed: &mut Parsed,
    ) -> Res<Function> {
        let list = d
            .get("Functions")
            .and_then(|v| resolved(doc, v))
            .ok_or(FunctionError)?;
        let items = list.as_array().ok_or(FunctionError)?;
        if items.is_empty() || items.len() > 256 {
            return Err(FunctionError);
        }
        let mut functions = Vec::new();
        for item in items {
            functions.push(Self::parse_at(doc, item, depth + 1, parsed)?);
        }
        let bounds = numbers(doc, d.get("Bounds")).ok_or(FunctionError)?;
        let encode = numbers(doc, d.get("Encode")).ok_or(FunctionError)?;
        if bounds.len() != functions.len() - 1 || encode.len() < functions.len() * 2 {
            return Err(FunctionError);
        }
        if functions
            .iter()
            .any(|f| f.inputs != 1 || f.outputs != functions[0].outputs)
        {
            return Err(FunctionError);
        }
        Ok(Function {
            inputs: 1,
            outputs: functions[0].outputs,
            domain: vec![],
            range: None,
            kind: Kind::Stitching {
                functions,
                bounds,
                encode,
            },
        })
    }

    fn sampled(
        doc: &mut Document,
        d: &lazy_pdf_syntax::Dict,
        stream: Option<&Stream>,
        domain: &[f64],
        range: Option<&[f64]>,
    ) -> Res<Function> {
        let inputs = domain.len() / 2;
        let (Some(stream), Some(range)) = (stream, range) else {
            return Err(FunctionError);
        };
        if range.len() < 2 || inputs > 8 {
            return Err(FunctionError);
        }
        let outputs = range.len() / 2;
        let bits = int_of(doc, d.get("BitsPerSample"), 0);
        let sizes = numbers(doc, d.get("Size")).ok_or(FunctionError)?;
        if sizes.len() != inputs || ![1, 2, 4, 8, 12, 16, 24, 32].contains(&bits) {
            return Err(FunctionError);
        }
        let bits = bits as usize;
        let mut size = vec![0usize; inputs];
        let mut stride = vec![0usize; inputs];
        let mut total = outputs;
        for i in 0..inputs {
            if sizes[i].is_nan() || sizes[i] < 1.0 || sizes[i] > i32::MAX as f64 {
                return Err(FunctionError);
            }
            size[i] = sizes[i] as usize;
            stride[i] = total.min(i32::MAX as usize);
            if size[i] as u64 > 8 * MAX_ARRAY_LENGTH as u64 / bits as u64 / total as u64 {
                return Err(FunctionError);
            }
            total = total.checked_mul(size[i]).ok_or(FunctionError)?;
        }
        let data = doc.stream_data(stream).map_err(|_| FunctionError)?;
        if total.checked_mul(bits).ok_or(FunctionError)? > data.len().saturating_mul(8) {
            return Err(FunctionError);
        }
        let mut encode = numbers(doc, d.get("Encode")).unwrap_or_default();
        if encode.len() < inputs * 2 {
            encode = size.iter().flat_map(|&s| [0.0, (s - 1) as f64]).collect();
        }
        let mut decode = numbers(doc, d.get("Decode")).unwrap_or_default();
        if decode.len() < outputs * 2 {
            decode = range.to_vec();
        }
        Ok(Function {
            inputs,
            outputs,
            domain: vec![],
            range: None,
            kind: Kind::Sampled {
                data,
                size,
                stride,
                bits,
                encode,
                decode,
            },
        })
    }

    fn post_script(
        doc: &mut Document,
        stream: Option<&Stream>,
        domain: &[f64],
        range: Option<&[f64]>,
    ) -> Res<Function> {
        let (Some(stream), Some(range)) = (stream, range) else {
            return Err(FunctionError);
        };
        if range.len() < 2 {
            return Err(FunctionError);
        }
        let text = doc.stream_data(stream).map_err(|_| FunctionError)?;
        let mut at = 0;
        skip_space(&text, &mut at);
        if at >= text.len() || text[at] != b'{' {
            return Err(FunctionError);
        }
        at += 1;
        let program = parse_block(&text, &mut at, 0)?;
        Ok(Function {
            inputs: domain.len() / 2,
            outputs: range.len() / 2,
            domain: vec![],
            range: None,
            kind: Kind::PostScript(program),
        })
    }

    /// `output` must hold `outputs` values.
    pub fn evaluate(&self, input: &[f64], output: &mut [f64]) -> Res<()> {
        let count = self.inputs.min(32);
        let mut clipped = vec![0.0; count];
        for (i, slot) in clipped.iter_mut().enumerate() {
            let v = input.get(i).copied().unwrap_or(0.0);
            *slot = clamp(v, self.domain[2 * i], self.domain[2 * i + 1]);
        }
        self.compute(&clipped, output)?;
        if let Some(range) = &self.range {
            for i in 0..self.outputs {
                output[i] = clamp(output[i], range[2 * i], range[2 * i + 1]);
            }
        }
        Ok(())
    }

    fn compute(&self, input: &[f64], output: &mut [f64]) -> Res<()> {
        match &self.kind {
            Kind::Combined(parts) => {
                let mut at = 0;
                for part in parts {
                    let mut piece = vec![0.0; part.outputs];
                    part.evaluate(input, &mut piece)?;
                    output[at..at + piece.len()].copy_from_slice(&piece);
                    at += piece.len();
                }
            }
            Kind::Exponential { c0, c1, n } => {
                let x = if *n == 1.0 {
                    input[0]
                } else {
                    input[0].powf(*n)
                };
                for i in 0..c0.len() {
                    output[i] = c0[i] + x * (c1[i] - c0[i]);
                }
            }
            Kind::Stitching {
                functions,
                bounds,
                encode,
            } => {
                let x = input[0];
                let mut i = 0;
                while i < bounds.len() && x >= bounds[i] {
                    i += 1;
                }
                let low = if i == 0 {
                    self.domain[0]
                } else {
                    bounds[i - 1]
                };
                let high = if i == bounds.len() {
                    self.domain[1]
                } else {
                    bounds[i]
                };
                let encoded = [interpolate(x, low, high, encode[2 * i], encode[2 * i + 1])];
                functions[i].evaluate(&encoded, output)?;
            }
            Kind::Sampled {
                data,
                size,
                stride,
                bits,
                encode,
                decode,
            } => self.compute_sampled(input, output, data, size, stride, *bits, encode, decode),
            Kind::PostScript(program) => {
                let mut stack = [Value::default(); STACK_SIZE];
                let mut top = 0;
                for &x in input {
                    stack[top] = Value {
                        number: x,
                        is_boolean: false,
                    };
                    top += 1;
                }
                run(program, &mut stack, &mut top)?;
                if top < self.outputs {
                    return Err(FunctionError);
                }
                for i in 0..self.outputs {
                    output[i] = stack[top - self.outputs + i].number;
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_sampled(
        &self,
        input: &[f64],
        output: &mut [f64],
        data: &[u8],
        size: &[usize],
        stride: &[usize],
        bits: usize,
        encode: &[f64],
        decode: &[f64],
    ) {
        let inputs = self.inputs;
        let mut low = vec![0usize; inputs];
        let mut fraction = vec![0.0; inputs];
        for i in 0..inputs {
            let e = interpolate(
                input[i],
                self.domain[2 * i],
                self.domain[2 * i + 1],
                encode[2 * i],
                encode[2 * i + 1],
            );
            // A NaN does not clamp and leaves a fraction that weighs the corner past the last sample.
            let e = if e.is_nan() {
                0.0
            } else {
                clamp(e, 0.0, (size[i] - 1) as f64)
            };
            low[i] = (e as usize).min(size[i].saturating_sub(2));
            fraction[i] = e - low[i] as f64;
        }
        output[..self.outputs].fill(0.0);
        let maximum = if bits == 32 {
            u32::MAX as f64
        } else {
            ((1u64 << bits) - 1) as f64
        };
        for corner in 0..1usize << inputs {
            let mut weight = 1.0;
            let mut index = 0;
            for i in 0..inputs {
                let upper = (corner >> i) & 1 != 0;
                weight *= if upper {
                    fraction[i]
                } else {
                    1.0 - fraction[i]
                };
                index += (low[i] + usize::from(upper)) * stride[i];
            }
            if weight == 0.0 {
                continue;
            }
            for (j, slot) in output.iter_mut().enumerate().take(self.outputs) {
                *slot += weight * sample(data, bits, index + j);
            }
        }
        for j in 0..self.outputs {
            output[j] = interpolate(output[j], 0.0, maximum, decode[2 * j], decode[2 * j + 1]);
        }
    }
}

fn sample(data: &[u8], bits: usize, index: usize) -> f64 {
    let bit = index * bits;
    let at = bit >> 3;
    let byte = |i: usize| data.get(i).copied().unwrap_or(0) as u64;
    let value = match bits {
        8 => byte(at),
        16 => byte(at) << 8 | byte(at + 1),
        24 => byte(at) << 16 | byte(at + 1) << 8 | byte(at + 2),
        32 => byte(at) << 24 | byte(at + 1) << 16 | byte(at + 2) << 8 | byte(at + 3),
        12 => {
            if bit & 7 == 0 {
                byte(at) << 4 | byte(at + 1) >> 4
            } else {
                (byte(at) & 15) << 8 | byte(at + 1)
            }
        }
        _ => byte(at) >> (8 - bits - (bit & 7)) & ((1 << bits) - 1),
    };
    value as f64
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Code {
    Push,
    Abs,
    Add,
    Atan,
    Ceiling,
    Cos,
    Cvi,
    Cvr,
    Div,
    Exp,
    Floor,
    Idiv,
    Ln,
    Log,
    Mod,
    Mul,
    Neg,
    Round,
    Sin,
    Sqrt,
    Sub,
    Truncate,
    And,
    Bitshift,
    Eq,
    False,
    Ge,
    Gt,
    Le,
    Lt,
    Ne,
    Not,
    Or,
    True,
    Xor,
    Copy,
    Dup,
    Exch,
    Index,
    Pop,
    Roll,
    If,
    IfElse,
}

#[derive(Debug)]
struct Op {
    code: Code,
    value: f64,
    body: Option<Vec<Op>>,
    otherwise: Option<Vec<Op>>,
}

impl Op {
    fn new(code: Code) -> Op {
        Op {
            code,
            value: 0.0,
            body: None,
            otherwise: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Value {
    number: f64,
    is_boolean: bool,
}

fn word_code(word: &str) -> Option<Code> {
    Some(match word {
        "abs" => Code::Abs,
        "add" => Code::Add,
        "atan" => Code::Atan,
        "ceiling" => Code::Ceiling,
        "cos" => Code::Cos,
        "cvi" => Code::Cvi,
        "cvr" => Code::Cvr,
        "div" => Code::Div,
        "exp" => Code::Exp,
        "floor" => Code::Floor,
        "idiv" => Code::Idiv,
        "ln" => Code::Ln,
        "log" => Code::Log,
        "mod" => Code::Mod,
        "mul" => Code::Mul,
        "neg" => Code::Neg,
        "round" => Code::Round,
        "sin" => Code::Sin,
        "sqrt" => Code::Sqrt,
        "sub" => Code::Sub,
        "truncate" => Code::Truncate,
        "and" => Code::And,
        "bitshift" => Code::Bitshift,
        "eq" => Code::Eq,
        "false" => Code::False,
        "ge" => Code::Ge,
        "gt" => Code::Gt,
        "le" => Code::Le,
        "lt" => Code::Lt,
        "ne" => Code::Ne,
        "not" => Code::Not,
        "or" => Code::Or,
        "true" => Code::True,
        "xor" => Code::Xor,
        "copy" => Code::Copy,
        "dup" => Code::Dup,
        "exch" => Code::Exch,
        "index" => Code::Index,
        "pop" => Code::Pop,
        "roll" => Code::Roll,
        _ => return None,
    })
}

fn is_space(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

fn skip_space(text: &[u8], at: &mut usize) {
    while *at < text.len() {
        if is_space(text[*at]) {
            *at += 1;
        } else if text[*at] == b'%' {
            while *at < text.len() && text[*at] != 10 && text[*at] != 13 {
                *at += 1;
            }
        } else {
            break;
        }
    }
}

fn parse_block(text: &[u8], at: &mut usize, depth: usize) -> Res<Vec<Op>> {
    if depth > MAX_NESTING {
        return Err(FunctionError);
    }
    let mut ops: Vec<Op> = Vec::new();
    let mut blocks: Vec<Vec<Op>> = Vec::new();
    loop {
        skip_space(text, at);
        if *at >= text.len() {
            return Err(FunctionError);
        }
        let b = text[*at];
        if b == b'}' {
            *at += 1;
            return Ok(ops);
        }
        if b == b'{' {
            *at += 1;
            blocks.push(parse_block(text, at, depth + 1)?);
            continue;
        }
        let start = *at;
        while *at < text.len()
            && !is_space(text[*at])
            && text[*at] != b'{'
            && text[*at] != b'}'
            && text[*at] != b'%'
        {
            *at += 1;
        }
        let word: String = text[start..*at]
            .iter()
            .map(|&b| if b < 128 { b as char } else { '?' })
            .collect();
        if let Some(number) = parse_number(&word) {
            let mut op = Op::new(Code::Push);
            op.value = number;
            ops.push(op);
        } else if word == "if" && !blocks.is_empty() {
            let mut op = Op::new(Code::If);
            op.body = blocks.pop();
            blocks.clear();
            ops.push(op);
        } else if word == "ifelse" && blocks.len() >= 2 {
            let mut op = Op::new(Code::IfElse);
            op.otherwise = blocks.pop();
            op.body = blocks.pop();
            blocks.clear();
            ops.push(op);
        } else if let Some(code) = word_code(&word) {
            ops.push(Op::new(code));
        } else {
            return Err(FunctionError);
        }
    }
}

// A sign, digits with a point, an exponent; "Infinity" and "NaN" too, as the reference's parser takes them.
fn parse_number(word: &str) -> Option<f64> {
    let b = word.as_bytes();
    let mut at = 0;
    if at < b.len() && (b[at] == b'+' || b[at] == b'-') {
        at += 1;
    }
    let rest = word[at..].to_ascii_lowercase();
    if rest == "infinity" {
        return Some(if b.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    if rest == "nan" {
        return Some(f64::NAN);
    }
    let mut digits = 0;
    while at < b.len() && b[at].is_ascii_digit() {
        at += 1;
        digits += 1;
    }
    if at < b.len() && b[at] == b'.' {
        at += 1;
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if at < b.len() && (b[at] == b'e' || b[at] == b'E') {
        at += 1;
        if at < b.len() && (b[at] == b'+' || b[at] == b'-') {
            at += 1;
        }
        let mut exponent = 0;
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
            exponent += 1;
        }
        if exponent == 0 {
            return None;
        }
    }
    if at == b.len() {
        word.parse().ok()
    } else {
        None
    }
}

fn need(required: usize, top: usize, push: usize) -> Res<()> {
    if top < required || top + push > STACK_SIZE {
        Err(FunctionError)
    } else {
        Ok(())
    }
}

fn flag(on: bool) -> Value {
    Value {
        number: if on { 1.0 } else { 0.0 },
        is_boolean: true,
    }
}

fn plain(number: f64) -> Value {
    Value {
        number,
        is_boolean: false,
    }
}

fn run(ops: &[Op], s: &mut [Value; STACK_SIZE], top: &mut usize) -> Res<()> {
    for op in ops {
        match op.code {
            Code::Push => {
                need(0, *top, 1)?;
                s[*top] = plain(op.value);
                *top += 1;
            }
            Code::True | Code::False => {
                need(0, *top, 1)?;
                s[*top] = flag(op.code == Code::True);
                *top += 1;
            }
            Code::If => {
                need(1, *top, 0)?;
                *top -= 1;
                if s[*top].number != 0.0
                    && let Some(body) = &op.body
                {
                    run(body, s, top)?;
                }
            }
            Code::IfElse => {
                need(1, *top, 0)?;
                *top -= 1;
                let branch = if s[*top].number != 0.0 {
                    &op.body
                } else {
                    &op.otherwise
                };
                if let Some(branch) = branch {
                    run(branch, s, top)?;
                }
            }
            Code::Pop => {
                need(1, *top, 0)?;
                *top -= 1;
            }
            Code::Dup => {
                need(1, *top, 1)?;
                s[*top] = s[*top - 1];
                *top += 1;
            }
            Code::Exch => {
                need(2, *top, 0)?;
                s.swap(*top - 1, *top - 2);
            }
            Code::Copy => {
                need(1, *top, 0)?;
                *top -= 1;
                let n = s[*top].number as i32;
                if n < 0 || n as usize > *top || *top + n as usize > STACK_SIZE {
                    return Err(FunctionError);
                }
                let n = n as usize;
                for i in 0..n {
                    s[*top + i] = s[*top - n + i];
                }
                *top += n;
            }
            Code::Index => {
                need(1, *top, 0)?;
                let n = s[*top - 1].number as i32;
                if n < 0 || n as usize >= *top - 1 {
                    return Err(FunctionError);
                }
                s[*top - 1] = s[*top - 2 - n as usize];
            }
            Code::Roll => {
                need(2, *top, 0)?;
                *top -= 1;
                let j = s[*top].number as i32;
                *top -= 1;
                let n = s[*top].number as i32;
                if n < 0 || n as usize > *top {
                    return Err(FunctionError);
                }
                if n > 1 {
                    let n = n as usize;
                    roll(s, *top - n, n, j);
                }
            }
            Code::Abs
            | Code::Ceiling
            | Code::Cos
            | Code::Cvi
            | Code::Cvr
            | Code::Floor
            | Code::Ln
            | Code::Log
            | Code::Neg
            | Code::Round
            | Code::Sin
            | Code::Sqrt
            | Code::Truncate
            | Code::Not => {
                need(1, *top, 0)?;
                s[*top - 1] = unary(op.code, s[*top - 1]);
            }
            _ => {
                need(2, *top, 0)?;
                s[*top - 2] = binary(op.code, s[*top - 2], s[*top - 1]);
                *top -= 1;
            }
        }
    }
    Ok(())
}

fn roll(s: &mut [Value], start: usize, length: usize, shift: i32) {
    let mut j = shift % length as i32;
    if j < 0 {
        j += length as i32;
    }
    if j == 0 {
        return;
    }
    let end = start + length;
    let j = j as usize;
    s[end - j..end].reverse();
    s[start..end - j].reverse();
    s[start..end].reverse();
}

fn unary(code: Code, v: Value) -> Value {
    let x = v.number;
    if code == Code::Not {
        return if v.is_boolean {
            flag(x == 0.0)
        } else {
            plain(!(x as i64) as f64)
        };
    }
    let result = match code {
        Code::Abs => x.abs(),
        Code::Ceiling => x.ceil(),
        Code::Cos => (x * std::f64::consts::PI / 180.0).cos(),
        Code::Cvi | Code::Truncate => x.trunc(),
        Code::Floor => x.floor(),
        Code::Ln => x.ln(),
        Code::Log => x.log10(),
        Code::Neg => -x,
        Code::Round => (x + 0.5).floor(),
        Code::Sin => (x * std::f64::consts::PI / 180.0).sin(),
        Code::Sqrt => x.sqrt(),
        _ => x,
    };
    plain(result)
}

fn binary(code: Code, a: Value, b: Value) -> Value {
    let (x, y) = (a.number, b.number);
    let logical = a.is_boolean && b.is_boolean;
    match code {
        Code::Eq => return flag(x == y),
        Code::Ne => return flag(x != y),
        Code::Gt => return flag(x > y),
        Code::Ge => return flag(x >= y),
        Code::Lt => return flag(x < y),
        Code::Le => return flag(x <= y),
        Code::And => {
            return if logical {
                flag(x != 0.0 && y != 0.0)
            } else {
                plain(((x as i64) & (y as i64)) as f64)
            };
        }
        Code::Or => {
            return if logical {
                flag(x != 0.0 || y != 0.0)
            } else {
                plain(((x as i64) | (y as i64)) as f64)
            };
        }
        Code::Xor => {
            return if logical {
                flag((x != 0.0) != (y != 0.0))
            } else {
                plain(((x as i64) ^ (y as i64)) as f64)
            };
        }
        _ => {}
    }
    let result = match code {
        Code::Add => x + y,
        Code::Sub => x - y,
        Code::Mul => x * y,
        Code::Div => {
            if y == 0.0 {
                0.0
            } else {
                x / y
            }
        }
        Code::Idiv => {
            let d = y as i64;
            if d == 0 {
                0.0
            } else if d == -1 {
                -((x as i64) as f64)
            } else {
                ((x as i64) / d) as f64
            }
        }
        Code::Mod => {
            let d = y as i64;
            if d == 0 || d == -1 {
                0.0
            } else {
                ((x as i64) % d) as f64
            }
        }
        Code::Exp => x.powf(y),
        Code::Atan => {
            let angle = x.atan2(y) * 180.0 / std::f64::consts::PI;
            if angle < 0.0 { angle + 360.0 } else { angle }
        }
        Code::Bitshift => {
            if y >= 0.0 {
                let shift = if y.is_nan() { y } else { y.min(62.0) } as i32;
                ((x as i64).wrapping_shl(shift as u32)) as f64
            } else {
                let shift = if -y > 62.0 { 62 } else { (-y) as i32 };
                ((x as i64) >> shift) as f64
            }
        }
        _ => 0.0,
    };
    plain(result)
}
