//! pdfaid:part and pdfaid:conformance from XMP metadata. The metadata is the file's own, so the reader takes no
//! DTD and expands no entity; anything that is not well-formed XML claims nothing.

/// Nesting deeper than any packet has; each open element collects its own text.
const MAX_DEPTH: usize = 256;

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

/// Candidates in document order; an element's own text is filled in when it closes.
type Candidates = Vec<Option<String>>;

fn parse(xml: &str) -> Option<(Option<String>, Option<String>)> {
    let bytes = xml.as_bytes();
    let mut at = 0;
    let mut stack: Vec<Open> = Vec::new();
    let mut bindings: Vec<(String, String)> = Vec::new();
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
                stack.iter_mut().for_each(|o| o.text.push_str(chunk));
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
            stack.iter_mut().for_each(|o| o.text.push_str(&body[..end]));
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
                    bindings.push((String::new(), value.clone()));
                } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                    bindings.push((prefix.to_string(), value.clone()));
                }
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

fn bound<'a>(bindings: &'a [(String, String)], prefix: &str) -> Option<&'a str> {
    bindings
        .iter()
        .rev()
        .find(|(p, _)| p == prefix)
        .map(|(_, uri)| uri.as_str())
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
