//! Which optional content is off in the default view (ISO 32000-1 8.11.2): the groups /OCProperties /D turns off, and
//! the membership dictionaries that follow from them.

use std::collections::HashSet;

use lazy_pdf_fonts::OptionalContent;
use lazy_pdf_syntax::{Document, Object};

const MAX_LAYERS: usize = 100_000;

#[derive(Debug, Default)]
pub struct LayerVisibility {
    off: HashSet<u32>,
}

fn numbers(doc: &mut Document, config: Option<&lazy_pdf_syntax::Dict>, key: &str) -> HashSet<u32> {
    let mut set = HashSet::new();
    let Some(config) = config else { return set };
    if let Some(Object::Array(items)) = doc.lookup(config, key).map(|v| v.into_shared()).as_deref()
    {
        for item in items.iter().take(MAX_LAYERS) {
            if let Object::Ref(r) = item {
                set.insert(r.number);
            }
        }
    }
    set
}

impl LayerVisibility {
    /// The groups the default configuration turns off, by object number. /BaseState, /ON and /OFF apply; /AS usage
    /// rules do not.
    pub fn new(doc: &mut Document) -> LayerVisibility {
        let mut visibility = LayerVisibility::default();
        let Some(catalog) = doc.catalog() else {
            return visibility;
        };
        let Some(catalog) = catalog.as_dict() else {
            return visibility;
        };
        let Some(properties) = doc.dict_of(catalog, "OCProperties") else {
            return visibility;
        };
        let config = doc.dict_of(&properties, "D");
        let base_off = config
            .as_ref()
            .and_then(|c| doc.name_of(c, "BaseState"))
            .is_some_and(|n| n == b"OFF");
        let on = numbers(doc, config.as_ref(), "ON");
        let off = numbers(doc, config.as_ref(), "OFF");
        let groups = match doc
            .lookup(&properties, "OCGs")
            .map(|v| v.into_shared())
            .as_deref()
        {
            Some(Object::Array(items)) => items.clone(),
            _ => return visibility,
        };
        let mut seen = HashSet::new();
        for item in groups.iter().take(MAX_LAYERS) {
            let Object::Ref(r) = item else { continue };
            if !seen.insert(r.number) {
                continue;
            }
            if !matches!(doc.resolve(item).as_deref(), Some(Object::Dict(_))) {
                continue;
            }
            let shown = if base_off {
                on.contains(&r.number)
            } else {
                !off.contains(&r.number)
            };
            if !shown && matches!(doc.get(r.number).as_deref(), Some(Object::Dict(_))) {
                visibility.off.insert(r.number);
            }
        }
        visibility
    }

    /// /VE expressions are not evaluated and count as visible.
    pub fn is_hidden(&self, group: &OptionalContent) -> bool {
        if group.id.is_some_and(|id| self.off.contains(&id)) {
            return true;
        }
        if !group.is_membership || group.members.is_empty() {
            return false;
        }
        let hidden = group
            .members
            .iter()
            .filter(|m| m.is_some_and(|n| self.off.contains(&n)))
            .count();
        match group.policy.as_str() {
            "AllOn" => hidden > 0,
            "AnyOff" => hidden == 0,
            "AllOff" => hidden < group.members.len(),
            _ => hidden == group.members.len(),
        }
    }
}
