//! pdfaid:part and pdfaid:conformance from XMP metadata. The metadata is the file's own, so the reader takes no
//! DTD and expands no entity; anything that is not well-formed XML claims nothing.

use std::collections::HashMap;

/// Nesting deeper than any packet has; a part or conformance element collects its own text.
const MAX_DEPTH: usize = 256;

const MAX_VALUE: usize = 256;

const PDFA_ID: &str = "http://www.aiim.org/pdfa/ns/id/";

/// The part and the conformance level an XMP packet claims, each as an attribute or an element of the pdfaid
/// namespace. The first one in document order counts. A level means nothing without a part.
pub(crate) fn read_pdfa_id(xmp: &[u8]) -> (Option<i32>, Option<String>) {
    let text = String::from_utf8_lossy(xmp);
    let start = text.find("<x:xmpmeta").or_else(|| text.find("<rdf:RDF"));
    let end = text
        .rfind("</x:xmpmeta>")
        .map(|at| at + "</x:xmpmeta>".len());
    let xml = match (start, end) {
        (Some(start), Some(end)) if end > start => &text[start..end],
        (Some(start), _) => &text[start..],
        _ => &text[..],
    };
    let (part, conformance) = parse(xml).unwrap_or_default();
    let part = part.and_then(|p| p.trim().parse::<i32>().ok());
    let level = conformance
        .map(|c| c.trim().to_uppercase())
        .filter(|c| !c.is_empty());
    (part, part.and(level))
}

struct Open {
    name: String,
    bindings: usize,
    part: Option<usize>,
    conformance: Option<usize>,
    text: String,
}

/// Text goes only to the elements that can be a part or a conformance, and only a few bytes of it: a value is short.
fn collect(stack: &mut [Open], chunk: &str) {
    for open in stack
        .iter_mut()
        .filter(|o| o.part.is_some() || o.conformance.is_some())
    {
        let mut take = chunk.len().min(MAX_VALUE.saturating_sub(open.text.len()));
        while !chunk.is_char_boundary(take) {
            take -= 1;
        }
        open.text.push_str(&chunk[..take]);
    }
}

/// Candidates in document order; an element's own text is filled in when it closes.
type Candidates = Vec<Option<String>>;

fn parse(xml: &str) -> Option<(Option<String>, Option<String>)> {
    let bytes = xml.as_bytes();
    let mut at = 0;
    let mut stack: Vec<Open> = Vec::new();
    let mut bindings = Bindings::default();
    let (mut parts, mut conformances): (Candidates, Candidates) = (Vec::new(), Vec::new());
    let mut roots = 0;
    while at < bytes.len() {
        if bytes[at] != b'<' {
            let next = xml[at..].find('<').map_or(bytes.len(), |n| at + n);
            let chunk = &xml[at..next];
            if stack.is_empty() {
                if !chunk.trim().is_empty() {
                    return None;
                }
            } else {
                collect(&mut stack, &decode(chunk)?);
            }
            at = next;
            continue;
        }
        let rest = &xml[at..];
        if let Some(body) = rest.strip_prefix("<!--") {
            at += 4 + body.find("-->")? + 3;
        } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>")?;
            if stack.is_empty() {
                return None;
            }
            collect(&mut stack, &body[..end]);
            at += 9 + end + 3;
        } else if rest.starts_with("<!") {
            return None;
        } else if let Some(body) = rest.strip_prefix("<?") {
            at += 2 + body.find("?>")? + 2;
        } else if let Some(body) = rest.strip_prefix("</") {
            let end = body.find('>')?;
            let open = stack.pop()?;
            if body[..end].trim_end() != open.name {
                return None;
            }
            bindings.truncate(open.bindings);
            if let Some(i) = open.part {
                parts[i] = Some(open.text.clone());
            }
            if let Some(i) = open.conformance {
                conformances[i] = Some(open.text);
            }
            at += 2 + end + 1;
        } else {
            let (name, attributes, empty, used) = start_tag(&rest[1..])?;
            at += 1 + used;
            let attributes = attributes
                .into_iter()
                .map(|(key, value)| Some((key, decode(&value)?)))
                .collect::<Option<Vec<_>>>()?;
            if stack.len() >= MAX_DEPTH {
                return None;
            }
            if stack.is_empty() {
                roots += 1;
                if roots > 1 {
                    return None;
                }
            }
            let kept = bindings.len();
            for (key, value) in &attributes {
                if key == "xmlns" {
                    bindings.push(String::new(), value.clone());
                } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                    bindings.push(prefix.to_string(), value.clone());
                }
            }
            let unbound = |prefix: &str| {
                !prefix.is_empty()
                    && prefix != "xml"
                    && prefix != "xmlns"
                    && bound(&bindings, prefix).is_none()
            };
            if unbound(name.split_once(':').map_or("", |(prefix, _)| prefix))
                || attributes.iter().any(|(key, _)| {
                    key.split_once(':')
                        .is_some_and(|(prefix, _)| unbound(prefix))
                })
            {
                return None;
            }
            for (key, value) in &attributes {
                match key.split_once(':') {
                    Some((prefix, "part")) if bound(&bindings, prefix) == Some(PDFA_ID) => {
                        parts.push(Some(value.clone()));
                    }
                    Some((prefix, "conformance")) if bound(&bindings, prefix) == Some(PDFA_ID) => {
                        conformances.push(Some(value.clone()));
                    }
                    _ => {}
                }
            }
            let mut open = Open {
                name: name.to_string(),
                bindings: kept,
                part: None,
                conformance: None,
                text: String::new(),
            };
            let (prefix, local) = name.split_once(':').unwrap_or(("", name));
            if bound(&bindings, prefix) == Some(PDFA_ID) {
                match local {
                    "part" => {
                        open.part = Some(parts.len());
                        parts.push(None);
                    }
                    "conformance" => {
                        open.conformance = Some(conformances.len());
                        conformances.push(None);
                    }
                    _ => {}
                }
            }
            if empty {
                bindings.truncate(kept);
                if let Some(i) = open.part {
                    parts[i] = Some(String::new());
                }
                if let Some(i) = open.conformance {
                    conformances[i] = Some(String::new());
                }
            } else {
                stack.push(open);
            }
        }
    }
    if !stack.is_empty() || roots == 0 {
        return None;
    }
    let first = |all: Candidates| all.into_iter().flatten().next();
    Some((first(parts), first(conformances)))
}

/// Text with its character references and the five predefined entities replaced; any other entity is an error,
/// since a packet declares none.
fn decode(text: &str) -> Option<String> {
    if !text.contains('&') {
        return Some(text.to_string());
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after.find(';')?;
        let name = &after[..end];
        match name {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let digits = name.strip_prefix('#')?;
                let code = match digits.strip_prefix('x') {
                    Some(hex) if hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
                        u32::from_str_radix(hex, 16).ok()?
                    }
                    None if digits.bytes().all(|b| b.is_ascii_digit()) => digits.parse().ok()?,
                    _ => return None,
                };
                out.push(char::from_u32(code).filter(|c| *c != ' ')?);
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Prefix bindings in scope, the innermost first for each prefix.
#[derive(Default)]
struct Bindings {
    order: Vec<String>,
    uris: HashMap<String, Vec<String>>,
}

impl Bindings {
    fn len(&self) -> usize {
        self.order.len()
    }

    fn push(&mut self, prefix: String, uri: String) {
        self.uris.entry(prefix.clone()).or_default().push(uri);
        self.order.push(prefix);
    }

    fn truncate(&mut self, len: usize) {
        while self.order.len() > len {
            let prefix = self.order.pop().unwrap();
            if let Some(uris) = self.uris.get_mut(&prefix) {
                uris.pop();
                if uris.is_empty() {
                    self.uris.remove(&prefix);
                }
            }
        }
    }
}

fn bound<'a>(bindings: &'a Bindings, prefix: &str) -> Option<&'a str> {
    bindings.uris.get(prefix)?.last().map(String::as_str)
}

type StartTag<'a> = (&'a str, Vec<(String, String)>, bool, usize);

/// The name and attributes of a start tag whose text follows the "<", whether it ends in "/>", and the bytes used.
fn start_tag(tag: &str) -> Option<StartTag<'_>> {
    let bytes = tag.as_bytes();
    let stop = |b: u8| b.is_ascii_whitespace() || b == b'/' || b == b'>';
    let name_end = bytes.iter().position(|&b| stop(b))?;
    if name_end == 0 {
        return None;
    }
    let mut at = name_end;
    let mut attributes = Vec::new();
    loop {
        while bytes.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        match bytes[at] {
            b'>' => return Some((&tag[..name_end], attributes, false, at + 1)),
            b'/' if bytes.get(at + 1) == Some(&b'>') => {
                return Some((&tag[..name_end], attributes, true, at + 2));
            }
            b'/' => return None,
            _ => {}
        }
        let key_end = at + bytes[at..].iter().position(|&b| b == b'=' || stop(b))?;
        let key = &tag[at..key_end];
        at = key_end;
        while bytes.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        if bytes[at] != b'=' {
            return None;
        }
        at += 1;
        while bytes.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        let quote = *bytes.get(at)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let value_end = at + 1 + bytes[at + 1..].iter().position(|&b| b == quote)?;
        attributes.push((key.to_string(), tag[at + 1..value_end].to_string()));
        at = value_end + 1;
    }
}
