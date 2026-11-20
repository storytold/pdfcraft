//! PDF 2.0 associated files (ISO 32000-2 §14.13): the `/AF` arrays of the document catalog, of
//! pages and of annotations. Each entry is a file specification dictionary (§7.11.3) whose `/EF`
//! holds the embedded file stream (§7.11.4, Table 43); `/AFRelationship` says how the file relates
//! to its owner. Form XObjects, structure elements and marked content can own associated files
//! too; they are not read here.
//!
//! The walk is read-only and bounded: references are followed by the structure's own bounded
//! dereference, each `/Annots` array and annotation is read once, an entry reachable from several
//! owners by the same object is listed once, and the entries read, the files listed and the notes
//! kept are all capped.

use std::panic::{AssertUnwindSafe, catch_unwind};

use super::core::load_options;
use super::model::{AssociatedFile, AssociatedFileOwner, AssociatedFileRelationship, MAX_ASSOCIATED_FILE_OWNERS};
use super::{Dictionary, DocInfo, Document, HashMap, HashSet, Inspector, Object, ObjectId};

/// At most this many associated files are listed.
pub(super) const MAX_ASSOCIATED_FILES: usize = 4096;
/// At most this many `/AF` entries are read from all owners together, duplicates included.
pub(super) const MAX_ASSOCIATED_FILE_ENTRIES: usize = 1 << 16;
/// At most this many notes are kept; the rest are counted in one last note.
pub(super) const MAX_ASSOCIATED_FILE_NOTES: usize = 64;
/// Decoded bytes one associated file may expand to, as for attachments: a decompression bomb
/// stops here.
const MAX_ASSOCIATED_FILE_BYTES: usize = 1 << 30;

/// What makes two `/AF` entries the same file: the object an indirect entry resolves to, or, for a
/// direct entry, its place in an indirect `/AF` array (which several owners can share).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Identity {
    Object(ObjectId),
    Slot(ObjectId, usize),
}

#[derive(Default)]
struct Walk {
    /// Where each file listed so far is in the list.
    listed: HashMap<Identity, usize>,
    /// Entries already skipped, so a shared one is noted once.
    skipped: HashSet<Identity>,
    /// `/AF` entries read so far.
    read: usize,
    /// A cap was reached: nothing more is read. Its note is always kept, after the others.
    stopped: Option<String>,
    notes: Vec<String>,
    notes_dropped: usize,
}

impl Walk {
    fn stop(&mut self, note: String) {
        self.stopped.get_or_insert(note);
    }

    fn note(&mut self, note: String) {
        if self.notes.len() < MAX_ASSOCIATED_FILE_NOTES {
            self.notes.push(note);
        } else {
            self.notes_dropped = self.notes_dropped.saturating_add(1);
        }
    }
}

/// The owner as the notes name it, pages counted from 1.
fn owner_text(owner: AssociatedFileOwner) -> String {
    match owner {
        AssociatedFileOwner::Document => "The document".into(),
        AssociatedFileOwner::Page { page } => format!("Page {}", page.saturating_add(1)),
        AssociatedFileOwner::Annotation { page, index } => {
            format!("Annotation {} on page {}", index.saturating_add(1), page.saturating_add(1))
        }
    }
}

impl<'a> Inspector<'a> {
    /// Fill `info.associated_files` and `info.associated_file_notes` from the `/AF` arrays of the
    /// catalog, of every page and of every page's annotations. `text_budget` is shared with the
    /// attachments' names and descriptions.
    pub(super) fn associated_files(&self, catalog: &'a Dictionary, info: &mut DocInfo, text_budget: &mut usize) {
        let mut walk = Walk::default();
        let mut files = Vec::new();
        self.owner_files(catalog, AssociatedFileOwner::Document, &mut files, &mut walk, text_budget);
        // An annotation, or a whole `/Annots` array, that several pages list is read on the first
        // of them, as the annotation list does.
        let mut seen_arrays = HashSet::new();
        let mut seen_annotations = HashSet::new();
        for (page, &id) in self.page_ids.iter().enumerate() {
            if walk.stopped.is_some() {
                break;
            }
            let Ok(page_dict) = self.doc.get_dictionary(id) else { continue };
            self.owner_files(page_dict, AssociatedFileOwner::Page { page }, &mut files, &mut walk, text_budget);
            let Ok(annots) = page_dict.get(b"Annots") else { continue };
            if let Some(r) = self.resolved_id(annots)
                && !seen_arrays.insert(r)
            {
                continue;
            }
            let Object::Array(annots) = self.resolve(annots) else { continue };
            for (index, a) in annots.iter().enumerate() {
                if walk.stopped.is_some() {
                    break;
                }
                if let Some(r) = self.resolved_id(a)
                    && !seen_annotations.insert(r)
                {
                    continue;
                }
                let Some(d) = self.dict(a) else { continue };
                // Not an annotation: its `/Type` is there and is not `/Annot` (§12.5.2, as the
                // annotation list reads it).
                if let Ok(t) = d.get(b"Type").map(|t| self.resolve(t))
                    && !matches!(t, Object::Null | Object::Reference(_))
                    && t.as_name().ok() != Some(b"Annot".as_slice())
                {
                    continue;
                }
                self.owner_files(d, AssociatedFileOwner::Annotation { page, index }, &mut files, &mut walk, text_budget);
            }
        }
        if walk.notes_dropped > 0 {
            walk.notes.push(format!("{} more notes about associated files are not shown.", walk.notes_dropped));
        }
        walk.notes.extend(walk.stopped.take());
        info.associated_files = files;
        info.associated_file_notes = walk.notes;
    }

    /// The files `owner_dict`'s `/AF` lists, added to `files` or noted in `walk`.
    fn owner_files(
        &self,
        owner_dict: &'a Dictionary,
        owner: AssociatedFileOwner,
        files: &mut Vec<AssociatedFile>,
        walk: &mut Walk,
        text_budget: &mut usize,
    ) {
        let Ok(value) = owner_dict.get(b"AF") else { return };
        let array_id = self.resolved_id(value);
        let entries = match self.resolve(value) {
            Object::Array(entries) => entries,
            // A null value is the same as leaving the entry out (§7.3.9).
            Object::Null => return,
            _ => {
                walk.note(format!("{}: /AF is not an array, so it was skipped.", owner_text(owner)));
                return;
            }
        };
        for (index, entry) in entries.iter().enumerate() {
            if walk.read >= MAX_ASSOCIATED_FILE_ENTRIES {
                walk.stop(format!("Only the first {MAX_ASSOCIATED_FILE_ENTRIES} /AF entries were read."));
                return;
            }
            walk.read += 1;
            // A reference that can't be followed is still told apart by the object it names.
            let identity = match (self.resolved_id(entry), entry) {
                (Some(id), _) => Some(Identity::Object(id)),
                (None, Object::Reference(id)) => Some(Identity::Object(*id)),
                (None, _) => array_id.map(|a| Identity::Slot(a, index)),
            };
            if let Some(identity) = identity {
                if let Some(&at) = walk.listed.get(&identity) {
                    if let Some(file) = files.get_mut(at)
                        && file.owners.len() < MAX_ASSOCIATED_FILE_OWNERS
                        && !file.owners.contains(&owner)
                    {
                        file.owners.push(owner);
                    }
                    continue;
                }
                if walk.skipped.contains(&identity) {
                    continue;
                }
            }
            match self.associated_file(entry, text_budget) {
                Ok((file, relationship_note)) => {
                    if files.len() >= MAX_ASSOCIATED_FILES {
                        walk.stop(format!("Only the first {MAX_ASSOCIATED_FILES} associated files are listed."));
                        return;
                    }
                    if let Some(why) = relationship_note {
                        walk.note(format!("{}, /AF entry {}: {why}", owner_text(owner), index.saturating_add(1)));
                    }
                    if let Some(identity) = identity {
                        walk.listed.insert(identity, files.len());
                    }
                    files.push(AssociatedFile { owners: vec![owner], index, ..file });
                }
                Err(why) => {
                    if let Some(identity) = identity {
                        walk.skipped.insert(identity);
                    }
                    walk.note(format!("{}, /AF entry {}: {why}, so it was skipped.", owner_text(owner), index.saturating_add(1)));
                }
            }
        }
    }

    /// One `/AF` entry read as a file (with no owner yet), and a note when its relationship was
    /// read with the default; `Err` says why it isn't one.
    fn associated_file(&self, entry: &'a Object, text_budget: &mut usize) -> Result<(AssociatedFile, Option<&'static str>), &'static str> {
        let (spec, stream) = self.associated_stream(entry)?;
        let (relationship, note) = match spec.get(b"AFRelationship").map(|o| self.resolve(o)) {
            Ok(Object::Name(name)) => (AssociatedFileRelationship::from_name(&String::from_utf8_lossy(name)), None),
            Err(_) | Ok(Object::Null) => (AssociatedFileRelationship::Unspecified, None),
            Ok(_) => (AssociatedFileRelationship::Unspecified, Some("/AFRelationship is not a name, so it was read as Unspecified.")),
        };
        let size = stream
            .dict
            .get(b"Params")
            .ok()
            .and_then(|p| self.dict(p))
            .and_then(|p| p.get(b"Size").ok())
            .and_then(|s| self.resolve(s).as_i64().ok())
            .and_then(|s| u64::try_from(s).ok());
        let file = AssociatedFile {
            owners: Vec::new(),
            index: 0,
            name: self.budgeted_text(spec, b"UF", text_budget).or_else(|| self.budgeted_text(spec, b"F", text_budget)),
            relationship,
            description: self.budgeted_text(spec, b"Desc", text_budget),
            mime: self.budgeted_text(&stream.dict, b"Subtype", text_budget),
            size,
        };
        Ok((file, note))
    }

    /// The file specification an `/AF` entry is and the embedded file stream under its `/EF`
    /// (`/UF`, else `/F`); `Err` says what is wrong with it.
    fn associated_stream(&self, entry: &'a Object) -> Result<(&'a Dictionary, &'a lopdf::Stream), &'static str> {
        let spec = match self.resolve(entry) {
            Object::Dictionary(d) => d,
            // What is left when a reference can't be followed: it is missing or loops.
            Object::Reference(_) => return Err("its reference leads to no object"),
            _ => return Err("it is not a file specification dictionary"),
        };
        let ef = match spec.get(b"EF").map(|o| self.resolve(o)) {
            Err(_) | Ok(Object::Null) => return Err("it has no embedded file (/EF)"),
            Ok(Object::Dictionary(ef)) => ef,
            Ok(_) => return Err("its /EF is not a dictionary"),
        };
        let stream = [b"UF".as_slice(), b"F"]
            .into_iter()
            .find_map(|key| match self.resolve(ef.get(key).ok()?) {
                Object::Stream(s) => Some(s),
                _ => None,
            })
            .ok_or("its /EF holds no embedded file stream")?;
        Ok((spec, stream))
    }
}

/// The decoded bytes of an associated file `inspect` listed, read from `bytes` (the same document)
/// through the first of its owners. Decoding stops at 1 GiB. A document opened without the
/// password its embedded files need (`/AuthEvent /EFOpen`, ISO 32000-2 §7.6.6) holds them
/// encrypted: a request for one is an error, never its encrypted bytes. Never panics.
pub fn associated_file_data(bytes: &[u8], password: Option<&str>, file: &AssociatedFile) -> Result<Vec<u8>, String> {
    let run = || -> Result<Vec<u8>, String> {
        if pdfcraft_cos::Document::open_with_password(std::sync::Arc::new(bytes.to_vec()), password).is_ok_and(|d| d.embedded_files_locked()) {
            return Err("the associated file is encrypted: open the document with its password to read it".into());
        }
        let doc = Document::load_mem_with_options(bytes, load_options(password)).map_err(|e| e.to_string())?;
        let insp = Inspector::new(&doc);
        let owner = file.owners.first().ok_or("the associated file names no owner")?;
        let page_dict = |page: usize| -> Result<&Dictionary, String> {
            let id = insp.page_ids.get(page).ok_or_else(|| format!("there is no page {}", page.saturating_add(1)))?;
            doc.get_dictionary(*id).map_err(|e| e.to_string())
        };
        let owner_dict: &Dictionary = match *owner {
            AssociatedFileOwner::Document => doc.catalog().map_err(|e| e.to_string())?,
            AssociatedFileOwner::Page { page } => page_dict(page)?,
            AssociatedFileOwner::Annotation { page, index } => {
                let annots = page_dict(page)?.get(b"Annots").map_err(|_| "the page has no annotations")?;
                let annotation = insp.resolve(annots).as_array().map_err(|e| e.to_string())?.get(index).ok_or("the annotation is not there")?;
                insp.dict(annotation).ok_or("the annotation is not a dictionary")?
            }
        };
        let af = owner_dict.get(b"AF").map_err(|_| "its owner has no /AF")?;
        let entry = insp.resolve(af).as_array().map_err(|e| e.to_string())?.get(file.index).ok_or("the /AF entry is not there")?;
        let (_, stream) = insp.associated_stream(entry)?;
        if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(MAX_ASSOCIATED_FILE_BYTES).map_err(|e| format!("the associated file could not be decoded: {e}"))
        } else {
            Ok(stream.content.clone())
        }
    };
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|p| Err(format!("reading the associated file crashed: {}", crate::raster::panic_message(&p))))
}
