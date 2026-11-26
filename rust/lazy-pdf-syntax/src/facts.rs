//! Facts about the file.

use crate::document::Document;
use crate::lexer::Lexer;
use crate::object::Object;

impl Document {
    /// The file is signed: its form says signatures exist (/SigFlags bit 1) or the catalog carries signature
    /// permissions (/Perms).
    pub fn is_signed(&mut self) -> bool {
        let catalog = self.catalog_dict();
        if self
            .dict_of(&catalog, "Perms")
            .is_some_and(|p| !p.is_empty())
        {
            return true;
        }
        let Some(form) = self.dict_of(&catalog, "AcroForm") else {
            return false;
        };
        self.integer_of(&form, "SigFlags")
            .is_some_and(|flags| flags & 1 != 0)
    }

    /// The file is certified: its catalog's /Perms has a /DocMDP signature.
    pub fn is_certified(&mut self) -> bool {
        let catalog = self.catalog_dict();
        let Some(perms) = self.dict_of(&catalog, "Perms") else {
            return false;
        };
        self.lookup(&perms, "DocMDP").is_some()
    }

    /// The form has XFA data.
    pub fn has_xfa(&mut self) -> bool {
        let catalog = self.catalog_dict();
        let Some(form) = self.dict_of(&catalog, "AcroForm") else {
            return false;
        };
        self.lookup(&form, "XFA").is_some()
    }

    /// The catalog says the file is tagged (/MarkInfo /Marked true).
    pub fn is_tagged(&mut self) -> bool {
        let catalog = self.catalog_dict();
        let Some(mark_info) = self.dict_of(&catalog, "MarkInfo") else {
            return false;
        };
        self.bool_of(&mark_info, "Marked") == Some(true)
    }

    /// The first object of the file holds a /Linearized dictionary (Annex F) whose /L is the length of the file. An
    /// update added after it makes the length differ: the file is still valid but no longer linearized, and so is
    /// a file cut short.
    pub fn is_linearized(&mut self) -> bool {
        if let Some(known) = self.linearized {
            return known;
        }
        let found = match self.linearization_dictionary() {
            Some(Object::Dict(d)) => {
                matches!(d.get("L"), Some(Object::Number(n)) if n.integer() == Some(self.data.len() as i64))
            }
            _ => false,
        };
        self.linearized = Some(found);
        found
    }

    fn linearization_dictionary(&self) -> Option<Object> {
        let mut lexer = Lexer::new(&self.data, self.header_version_end);
        lexer.read_object_header().ok()?;
        match lexer.read_object(true).ok()? {
            Object::Dict(d) if d.contains_key("Linearized") => Some(Object::Dict(d)),
            _ => None,
        }
    }
}
