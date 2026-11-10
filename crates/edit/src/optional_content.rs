//! View visibility for shape extraction, following the renderer's default configuration and
//! View auto-state precedence (render/inspect/resources.rs, ISO 32000-2 §8.11.4).

use pdfcraft_cos::{Document, ObjRef, Object};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

fn read<T>(doc: &Document, object: &Object, f: impl FnOnce(&Object) -> T) -> T {
    if object.as_ref().is_some() { f(&doc.resolve(object)) } else { f(object) }
}

/// Reads page artwork and widget appearances from one immutable document. The default
/// optional-content configuration is resolved at most once; evaluation budgets and caches
/// belong to each extraction. Borrowing the document prevents stale configuration after edits.
pub struct ArtworkReader<'a> {
    pub(crate) doc: &'a Document,
    configuration: OnceLock<Configuration>,
}

impl<'a> ArtworkReader<'a> {
    pub fn new(doc: &'a Document) -> Self {
        Self { doc, configuration: OnceLock::new() }
    }

    pub(crate) fn visibility(&self) -> Visibility<'_> {
        let configuration = self.configuration.get_or_init(|| Configuration::new(self.doc));
        Visibility { doc: self.doc, configuration, cache: HashMap::new(), work: MAX_WORK, truncated: configuration.truncated }
    }
}

const MAX_WORK: usize = 100_000;

#[cfg(test)]
thread_local! {
    pub(crate) static CONFIGURATION_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct Configuration {
    off: HashSet<ObjRef>,
    work: usize,
    truncated: bool,
}

impl Configuration {
    fn new(doc: &Document) -> Self {
        #[cfg(test)]
        CONFIGURATION_BUILDS.set(CONFIGURATION_BUILDS.get() + 1);
        let mut result = Self { off: HashSet::new(), work: MAX_WORK, truncated: false };
        let Some(root) = doc.root() else { return result };
        let cat = doc.get(root);
        if let Some(props) = cat.as_dict().and_then(|cat| cat.get(b"OCProperties")) {
            read(doc, props, |props| {
                let Some(props) = props.as_dict() else { return };
                let Some(config) = props.get(b"D") else { return };
                read(doc, config, |config| {
                    let Some(config) = config.as_dict() else { return };
                    let groups = result.references(doc, props.get(b"OCGs"));
                    let known: HashSet<_> = groups.iter().copied().collect();
                    if config.name(b"BaseState") == Some(b"OFF") {
                        result.off.extend(groups);
                    }
                    for on in result.references(doc, config.get(b"ON")) {
                        result.off.remove(&on);
                    }
                    let off = result.references(doc, config.get(b"OFF"));
                    result.off.extend(off);
                    let mut auto = HashMap::new();
                    if let Some(apps) = config.get(b"AS") {
                        read(doc, apps, |apps| {
                            if let Some(apps) = apps.as_array() {
                                for app in apps {
                                    if !result.charge() {
                                        break;
                                    }
                                    read(doc, app, |app| {
                                        let Some(app) = app.as_dict() else { return };
                                        if app.name(b"Event") != Some(b"View") {
                                            return;
                                        }
                                        let view = app.get(b"Category").is_some_and(|v| {
                                            read(doc, v, |v| v.as_array().is_some_and(|a| a.iter().take(32).any(|n| n.as_name() == Some(b"View"))))
                                        });
                                        if !view {
                                            return;
                                        }
                                        for group in result.references(doc, app.get(b"OCGs")) {
                                            if !known.contains(&group) {
                                                continue;
                                            }
                                            let object = doc.get(group);
                                            let state = object.as_dict().and_then(|g| g.get(b"Usage")).and_then(|u| {
                                                read(doc, u, |u| {
                                                    u.as_dict()?.get(b"View").and_then(|v| {
                                                        read(doc, v, |v| match v.as_dict()?.name(b"ViewState") {
                                                            Some(b"ON") => Some(true),
                                                            Some(b"OFF") => Some(false),
                                                            _ => None,
                                                        })
                                                    })
                                                })
                                            });
                                            if let Some(state) = state {
                                                *auto.entry(group).or_insert(true) &= state;
                                            }
                                        }
                                    });
                                }
                            }
                        });
                    }
                    for (group, visible) in auto {
                        if visible {
                            result.off.remove(&group);
                        } else {
                            result.off.insert(group);
                        }
                    }
                });
            });
        }
        result
    }

    fn charge(&mut self) -> bool {
        if self.work == 0 {
            self.truncated = true;
            return false;
        }
        self.work -= 1;
        true
    }

    fn references(&mut self, doc: &Document, value: Option<&Object>) -> Vec<ObjRef> {
        let Some(value) = value else { return Vec::new() };
        read(doc, value, |value| {
            let Some(values) = value.as_array() else { return Vec::new() };
            values.iter().take_while(|_| self.charge()).filter_map(Object::as_ref).collect()
        })
    }
}

pub(crate) struct Visibility<'a> {
    doc: &'a Document,
    configuration: &'a Configuration,
    cache: HashMap<ObjRef, bool>,
    work: usize,
    pub truncated: bool,
}

impl Visibility<'_> {
    fn charge(&mut self) -> bool {
        if self.work == 0 {
            self.truncated = true;
            return false;
        }
        self.work -= 1;
        true
    }

    pub fn visible(&mut self, object: &Object) -> bool {
        if self.truncated {
            return false;
        }
        if let Some(r) = object.as_ref()
            && let Some(visible) = self.cache.get(&r)
        {
            return *visible;
        }
        let visible = self.evaluate(object, 0).unwrap_or(false);
        if let Some(r) = object.as_ref()
            && self.cache.len() < 20_000
        {
            self.cache.insert(r, visible);
        }
        visible
    }

    fn evaluate(&mut self, object: &Object, depth: usize) -> Option<bool> {
        if depth > 64 || !self.charge() {
            self.truncated = true;
            return None;
        }
        read(self.doc, object, |value| match value {
            Object::Dict(dict) if dict.name(b"Type") == Some(b"OCG") => Some(object.as_ref().is_none_or(|r| !self.configuration.off.contains(&r))),
            Object::Dict(dict) if dict.name(b"Type") == Some(b"OCMD") => {
                if let Some(expression) = dict.get(b"VE") {
                    return self.evaluate(expression, depth + 1);
                }
                let Some(groups) = dict.get(b"OCGs") else { return Some(true) };
                let (mut any, mut all) = (false, true);
                read(self.doc, groups, |values| {
                    let values = values.as_array().map(Vec::as_slice).unwrap_or(std::slice::from_ref(groups));
                    for group in values {
                        let on = self.evaluate(group, depth + 1)?;
                        any |= on;
                        all &= on;
                    }
                    match dict.name(b"P") {
                        None | Some(b"AnyOn") => Some(any),
                        Some(b"AllOn") => Some(all),
                        Some(b"AnyOff") => Some(!all),
                        Some(b"AllOff") => Some(!any),
                        _ => None,
                    }
                })
            }
            Object::Array(values) => {
                let (operator, args) = values.split_first()?;
                let mut result = operator.as_name() == Some(b"And");
                match operator.as_name() {
                    Some(b"Not") if args.len() == 1 => Some(!self.evaluate(args.first()?, depth + 1)?),
                    Some(b"And" | b"Or") if !args.is_empty() => {
                        for arg in args {
                            let on = self.evaluate(arg, depth + 1)?;
                            if operator.as_name() == Some(b"And") {
                                result &= on;
                            } else {
                                result |= on;
                            }
                        }
                        Some(result)
                    }
                    _ => None,
                }
            }
            _ => Some(true),
        })
    }
}

/// Marked-content nesting is not part of q/Q graphics state. Each Form call gets its own
/// stack while inheriting whether its caller draws at all.
#[derive(Default)]
pub(crate) struct Marked {
    stack: Vec<bool>,
    excess: usize,
    pub truncated: bool,
}
impl Marked {
    pub fn visible(&self) -> bool {
        self.excess == 0 && self.stack.last().copied().unwrap_or(true)
    }
    pub fn update(&mut self, op: &pdfcraft_content::Op, props: &pdfcraft_cos::Dict, visibility: &mut Visibility<'_>) -> bool {
        match op.op.as_slice() {
            b"BDC" | b"BMC" => {
                if self.stack.len() == 256 || self.excess > 0 {
                    self.excess = self.excess.saturating_add(1);
                    self.truncated = true;
                } else {
                    let visible = self.visible()
                        && (op.op != b"BDC"
                            || op.name(0) != Some(b"OC")
                            || op.operands.get(1).is_none_or(|p| visibility.visible(p.as_name().and_then(|n| props.get(n)).unwrap_or(p))));
                    self.stack.push(visible);
                }
                true
            }
            b"EMC" => {
                if self.excess > 0 {
                    self.excess -= 1;
                } else {
                    self.stack.pop();
                }
                true
            }
            _ => false,
        }
    }
}
