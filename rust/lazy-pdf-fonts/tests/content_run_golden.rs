// The run and nest lines of the content golden: pages run by the interpreter and what it reports. The fixture
// lines open encrypted files and wait for the crypt layer; they are skipped and counted.

mod common;

use common::pdf::TestPdf;
use common::{golden, golden_bytes, n};
use lazy_pdf_fonts::{
    ContentContext, ContentInterpreter, ContentVisitor, FormInfo, GlyphInfo, GraphicsState,
    ImageInfo, MarkedContent, Matrix, PathInfo, PathSegment, PathSegmentKind, PdfColor,
    ShadingInfo, StandardFilters,
};
use lazy_pdf_syntax::{Document, Object, Rect};

fn unescape(text: &str) -> Vec<u8> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        match b[i] {
            b'n' => out.push(10),
            b'r' => out.push(13),
            b't' => out.push(9),
            b'x' => {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            other => out.push(other),
        }
        i += 1;
    }
    out
}

fn latin(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

// {N*text} stands for text N times.
fn expand(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let close = chars[j.min(chars.len())..]
                .iter()
                .position(|&c| c == '}')
                .map(|p| p + j);
            if j > i + 1
                && j < chars.len()
                && chars[j] == '*'
                && let Some(close) = close
            {
                let count: usize = chars[i + 1..j]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                let unit: String = chars[j + 1..close].iter().collect();
                result += &unit.repeat(count);
                i = close + 1;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

fn flag(v: bool) -> &'static str {
    if v { "1" } else { "0" }
}

fn uni(text: Option<&str>) -> String {
    match text {
        None => "-".into(),
        Some(t) => t
            .chars()
            .map(|c| format!("{:04X}", c as u32))
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn mat(m: &Matrix) -> String {
    format!(
        "[{} {} {} {} {} {}]",
        n(m.a),
        n(m.b),
        n(m.c),
        n(m.d),
        n(m.e),
        n(m.f)
    )
}

fn rect(r: &Rect) -> String {
    format!(
        "[{} {} {} {}]",
        n(r.left),
        n(r.bottom),
        n(r.right),
        n(r.top)
    )
}

fn color_text(c: &PdfColor) -> String {
    let rgb = c.rgb();
    let numbers: Vec<String> = c.components.iter().map(|&v| n(v)).collect();
    let mut text = format!(
        "{}({}):{},{},{}",
        c.space.family,
        numbers.join(","),
        rgb.r,
        rgb.g,
        rgb.b
    );
    if let Some(name) = &c.pattern_name {
        text += &format!(":{}", name.text());
    }
    text
}

fn state_text(s: &GraphicsState) -> String {
    let dash: Vec<String> = s.dash.iter().map(|&v| n(v)).collect();
    format!(
        "ctm={} clip={} fill={} stroke={} lw={} cap={} join={} miter={} dash=[{}]@{} alpha={}/{} bm={} sm={} tc={} tw={} tz={} tl={} ts={} tr={} font={} size={}",
        mat(&s.ctm),
        rect(&s.clip),
        color_text(&s.fill_color),
        color_text(&s.stroke_color),
        n(s.line_width),
        s.line_cap,
        s.line_join,
        n(s.miter_limit),
        dash.join(","),
        n(s.dash_phase),
        n(s.fill_alpha),
        n(s.stroke_alpha),
        s.blend_mode,
        flag(s.soft_mask.is_some()),
        n(s.char_spacing),
        n(s.word_spacing),
        n(s.horizontal_scale),
        n(s.leading),
        n(s.rise),
        s.render_mode,
        s.font.as_ref().map_or("-", |f| f.name.as_str()),
        n(s.font_size)
    )
}

fn segments_text(segments: &[PathSegment]) -> String {
    segments
        .iter()
        .map(|s| match s.kind {
            PathSegmentKind::Move => format!("M {} {}", n(s.x1), n(s.y1)),
            PathSegmentKind::Line => format!("L {} {}", n(s.x1), n(s.y1)),
            PathSegmentKind::Curve => format!(
                "C {} {} {} {} {} {}",
                n(s.x1),
                n(s.y1),
                n(s.x2),
                n(s.y2),
                n(s.x3),
                n(s.y3)
            ),
            PathSegmentKind::Close => "Z".to_string(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[derive(Default)]
struct Dump {
    pending: Option<String>,
    repeats: usize,
    lines: Vec<String>,
    paths: usize,
    max_depth: usize,
}

impl Dump {
    fn flush(&mut self) {
        if let Some(p) = self.pending.take() {
            self.lines.push(if self.repeats > 1 {
                format!("{p} x{}", self.repeats)
            } else {
                p
            });
        }
    }

    fn add(&mut self, line: String) {
        self.flush();
        self.lines.push(line);
    }

    // Runs of the same event are one line: a thousand marked-content levels are not a thousand lines.
    fn event(&mut self, line: String) {
        if self.pending.as_deref() == Some(line.as_str()) {
            self.repeats += 1;
            return;
        }
        self.flush();
        self.pending = Some(line);
        self.repeats = 1;
    }
}

fn at(c: &ContentContext) -> String {
    format!(
        "op={} forms={} mcid={}",
        c.operator_index,
        c.forms.len(),
        c.mcid().map_or("-".to_string(), |m| m.to_string())
    )
}

impl ContentVisitor for Dump {
    fn show_glyph(&mut self, context: &ContentContext, glyph: &GlyphInfo) {
        let c = &glyph.character;
        self.max_depth = self.max_depth.max(context.forms.len());
        let line = format!(
            "glyph {} code={} len={} cid={} w={} u={} space={} va={} ox={} oy={} m={} box={} adv={} el={} off={} {}",
            at(context),
            c.code,
            c.length,
            c.cid,
            n(c.width),
            uni(c.unicode.as_deref()),
            flag(c.is_space),
            n(c.vertical_advance),
            n(c.origin_x),
            n(c.origin_y),
            mat(&glyph.matrix),
            rect(&glyph.bounds),
            n(glyph.advance),
            glyph.element,
            glyph.offset,
            state_text(&context.state)
        );
        self.add(line);
    }

    fn paint_path(&mut self, context: &ContentContext, path: &PathInfo) {
        self.paths += 1;
        self.max_depth = self.max_depth.max(context.forms.len());
        let line = format!(
            "path {} {} segs={} bounds={} ink={} fill={} stroke={} eo={} clips={} {}",
            at(context),
            path.operator,
            segments_text(&path.segments),
            rect(&path.bounds),
            rect(&path.ink_bounds),
            flag(path.fill),
            flag(path.stroke),
            flag(path.even_odd),
            flag(path.clips),
            state_text(&context.state)
        );
        self.add(line);
    }

    fn paint_image(&mut self, context: &ContentContext, image: &ImageInfo) {
        let line = format!(
            "image {} name={} inline={} stream={} w={} h={} mask={} oc={} m={} bounds={} dict={}",
            at(context),
            image.name.as_deref().unwrap_or("-"),
            flag(image.inline.is_some()),
            flag(image.stream.is_some()),
            image.width,
            image.height,
            flag(image.is_mask),
            flag(image.optional_content.is_some()),
            mat(&image.matrix),
            rect(&image.bounds),
            image.dictionary
        );
        self.add(line);
    }

    fn paint_shading(&mut self, context: &ContentContext, shading: &ShadingInfo) {
        let line = format!(
            "shading {} name={} bounds={} kind={}",
            at(context),
            shading.name,
            rect(&shading.bounds),
            if matches!(&*shading.shading, Object::Stream(_)) {
                "stream"
            } else {
                "dictionary"
            }
        );
        self.add(line);
    }

    fn begin_marked(&mut self, context: &ContentContext, marked: &MarkedContent) {
        self.event(format!(
            "begin {} tag={} mcid={} props={} oc={}",
            at(context),
            marked.tag,
            marked.mcid.map_or("-".to_string(), |m| m.to_string()),
            marked
                .properties
                .as_ref()
                .map_or("-".to_string(), ToString::to_string),
            flag(marked.optional_content.is_some())
        ));
    }

    fn end_marked(&mut self, context: &ContentContext, marked: &MarkedContent) {
        self.event(format!("end {} tag={}", at(context), marked.tag));
    }

    fn begin_form(&mut self, context: &ContentContext, form: &FormInfo) {
        let line = format!(
            "{} {} name={} m={} bounds={} oc={}",
            if form.is_pattern { "pattern" } else { "form" },
            at(context),
            form.name.as_deref().unwrap_or("-"),
            mat(&form.matrix),
            rect(&form.bounds),
            flag(form.optional_content.is_some())
        );
        self.add(line);
    }

    fn end_form(&mut self, context: &ContentContext, _form: &FormInfo) {
        self.add(format!("endform {}", at(context)));
    }
}

fn add_objects(pdf: &mut TestPdf, fields: &[&str]) {
    for (i, raw) in fields.iter().enumerate() {
        let number = 5 + i as u32;
        let field = expand(raw);
        let Some(tail) = field.strip_prefix("stream") else {
            pdf.obj(number, &latin(&unescape(&field)));
            continue;
        };
        let Some((entries, data)) = tail.split_once(" :: ") else {
            continue;
        };
        let entries = latin(&unescape(entries));
        let content = match data.strip_prefix("file:") {
            Some(path) => golden_bytes(path),
            None => unescape(data),
        };
        pdf.stream(number, entries.trim_matches(' '), &content);
    }
}

fn page(content: &[u8], resources: &str, more: impl FnOnce(&mut TestPdf)) -> Document {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
    pdf.obj(
        2,
        "<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800]>>",
    );
    pdf.obj(
        3,
        &format!("<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <<{resources}>>>>"),
    );
    pdf.stream(4, "", content);
    more(&mut pdf);
    Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap()
}

fn run_page(rest: &str) -> Vec<String> {
    let fields: Vec<&str> = rest.split(" ;; ").collect();
    let resources = latin(&unescape(&expand(fields[0])));
    let mut doc = page(&unescape(&expand(fields[1])), &resources, |pdf| {
        add_objects(pdf, &fields[2..])
    });
    let pages = doc.pages();
    let mut dump = Dump::default();
    let truncated = {
        let mut interpreter = ContentInterpreter::new(&mut doc, &mut dump);
        interpreter.run_page(&pages[0]);
        interpreter.is_truncated()
    };
    dump.flush();
    dump.lines.push(format!("truncated {}", flag(truncated)));
    dump.lines
}

fn nest(count: usize) -> Vec<String> {
    let names: Vec<String> = (0..count).map(|i| format!("/F{i} {} 0 R", 5 + i)).collect();
    let resources = format!("/XObject <<{}>>", names.join(" "));
    let mut doc = page(b"/F0 Do", &resources, |pdf| {
        for i in 0..count {
            pdf.stream(
                5 + i as u32,
                "/Type /XObject /Subtype /Form /BBox [0 0 100 100]",
                format!("0 0 1 1 re f /F{} Do", i + 1).as_bytes(),
            );
        }
    });
    let pages = doc.pages();
    let mut dump = Dump::default();
    ContentInterpreter::new(&mut doc, &mut dump).run_page(&pages[0]);
    vec![format!("paths {} depth {}", dump.paths, dump.max_depth)]
}

#[test]
fn runs_are_the_golden_file() {
    let mut actual = String::new();
    let mut skipped = 0;
    for line in golden("content/content-input.txt").split('\n') {
        let results = if let Some(rest) = line.strip_prefix("run ") {
            run_page(rest)
        } else if let Some(rest) = line.strip_prefix("nest ") {
            nest(rest.parse().unwrap())
        } else if line.starts_with("fixture ") {
            skipped += 1;
            continue;
        } else {
            continue;
        };
        actual += &format!("> {line}\n");
        for result in results {
            actual += &format!("< {result}\n");
        }
    }
    let mut expected = String::new();
    let mut inside = false;
    for line in golden("content/content-output.txt").split_inclusive('\n') {
        if line.starts_with("> ") {
            inside = line.starts_with("> run ") || line.starts_with("> nest ");
        }
        if inside {
            expected += line;
        }
    }
    assert!(expected.len() > 10_000);
    // All eight fixture lines open encrypted files.
    assert_eq!(skipped, 8);
    assert_eq!(
        actual.split('\n').count(),
        expected.split('\n').count(),
        "line count"
    );
    for (i, (a, e)) in actual.split('\n').zip(expected.split('\n')).enumerate() {
        assert_eq!(a, e, "line {}", i + 1);
    }
}
