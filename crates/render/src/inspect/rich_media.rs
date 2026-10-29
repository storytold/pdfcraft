//! Inventory RichMedia embedded assets; never activate a configuration or execute its content.

use super::core::spend;
use super::model::AttachmentSource;
use super::{Dictionary, DocInfo, HashSet, Inspector, Object};

/// A single explanation for content whose interactive Flash display cannot be drawn.
pub const LEGACY_FLASH_WARNING: &str = "This PDF contains legacy Flash/SWF content that PdfCraft does not play. Diagrams may be blank. Save embedded assets from the Attachments panel for external viewing or conversion.";

const MAX_MEDIA_ASSETS: usize = 1024;
const MAX_MEDIA_VISITS: usize = 8192;

pub(super) struct MediaBudget {
    pub visits: usize,
    pub assets: usize,
}

impl Default for MediaBudget {
    fn default() -> Self {
        Self { visits: MAX_MEDIA_VISITS, assets: MAX_MEDIA_ASSETS }
    }
}

impl<'a> Inspector<'a> {
    pub(super) fn rich_media(
        &self,
        info: &mut DocInfo,
        page: usize,
        index: usize,
        annot: &'a Dictionary,
        text_budget: &mut usize,
        budget: &mut MediaBudget,
    ) {
        if budget.visits == 0 || budget.assets == 0 {
            return;
        }
        budget.visits = budget.visits.saturating_sub(1);
        let Some(content) = annot.get(b"RichMediaContent").ok().and_then(|v| self.dict(v)) else { return };
        if let Ok(Object::Array(configs)) = content.get(b"Configurations").map(|v| self.resolve(v)) {
            for config in configs {
                if budget.visits == 0 {
                    break;
                }
                budget.visits = budget.visits.saturating_sub(1);
                if self
                    .dict(config)
                    .and_then(|d| d.get(b"Subtype").ok())
                    .is_some_and(|v| matches!(self.resolve(v), Object::Name(n) if n.as_slice() == b"Flash"))
                {
                    info.has_legacy_flash = true;
                }
            }
        }
        let Some(tree) = content.get(b"Assets").ok().and_then(|v| self.dict(v)) else { return };
        let mut seen = HashSet::new();
        let mut keys = HashSet::new();
        for entry in self.name_tree_limited(tree, &mut seen, 0, text_budget, &mut budget.visits) {
            if budget.assets == 0 {
                break;
            }
            // Name-tree keys must be strings. Keep display de-duplication, but extract by the
            // exact raw key: an earlier decoded alias may have been omitted by the text budget.
            let Some(raw_key) = entry.raw_key else { continue };
            let (key, spec) = (entry.key, entry.value);
            let fallback = |left: &mut usize| spend(key.as_bytes(), left, |_| key.clone());
            let att = self.attachment(spec, AttachmentSource::RichMedia { page, index, raw_key: raw_key.to_vec() }, text_budget, fallback);
            let swf = |name: &str| name.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("swf"));
            let embedded = self
                .dict(spec)
                .and_then(|d| d.get(b"EF").ok())
                .and_then(|v| self.dict(v))
                .and_then(|d| d.get(b"UF").ok().or_else(|| d.get(b"F").ok()))
                .map(|v| self.resolve(v));
            let flash_type = embedded.is_some_and(|v| match v {
                Object::Stream(s) => s
                    .dict
                    .get(b"Subtype")
                    .ok()
                    .is_some_and(|v| matches!(self.resolve(v), Object::Name(n) if n.as_slice() == b"application/x-shockwave-flash")),
                _ => false,
            });
            info.has_legacy_flash |= swf(&key) || swf(&att.name) || flash_type;
            // A duplicate display key can still name Flash content; only the listing is deduplicated.
            if !keys.insert(key.clone()) {
                continue;
            }
            budget.assets = budget.assets.saturating_sub(1);
            info.attachments.push(att);
        }
    }
}
