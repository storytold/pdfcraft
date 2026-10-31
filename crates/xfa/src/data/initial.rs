//! Preserve the instance structure created by an empty data merge before the first edit.

use std::collections::HashMap;

use super::{DataOp, DataPath, DatasetsWrite, Document, Template, XfaError, write_data_ops};
use crate::model::{Node, Occur, SomIndices, Subform};

/// Record initially instantiated groups, including empty groups. Later data merges must use
/// actual group counts rather than rerun `initial` (which would resurrect removed rows).
/// Existing data is never filled out with initial instances. Ambiguous same-name bindings
/// whose initial counts cannot survive a data merge are rejected before anything is written.
pub(crate) fn preserve_initial_instances(doc: &mut Document, tpl: &Template) -> Result<DatasetsWrite, XfaError> {
    if crate::data_of(doc).is_some_and(|data| !data.children.is_empty()) {
        return Ok(DatasetsWrite::default());
    }
    let mut scan = InitialNeed { visited: 0 };
    let mut needed = scan.subform(&tpl.root, 0)?;
    if !needed && let Some(pages) = &tpl.root.page_set {
        for area in &pages.areas {
            if scan.children(&area.items, 1)? {
                needed = true;
                break;
            }
        }
    }
    if !needed {
        // The minimum floor already keeps all these instances, so leave their packets alone.
        return Ok(DatasetsWrite::default());
    }
    if let Some(root_name) = &tpl.root.common.name
        && tpl.root.page_set.as_ref().is_some_and(|pages| pages.areas.iter().any(|area| area.name.as_ref() == Some(root_name)))
    {
        // data_path would enter the body template for this master's SOM. Its area
        // transparency then depends on unrelated body children rather than the master.
        return Err(XfaError::Malformed("The initial XFA instances use an ambiguous shared body-root and page-master name".into()));
    }
    let mut w = InitialGroups { ops: Vec::new(), counts: HashMap::new(), required: Vec::new(), visited: 0, plan_bytes: 0 };
    // Without a named body root, data_path cannot enter its template lookup; named
    // areas remain data segments just as they do under a page master.
    w.subform(&tpl.root, &Vec::new(), 0, 0, tpl.root.common.name.is_none())?;
    // Masters are separate SOM scopes in the current binding model, outside the body root.
    if let Some(pages) = &tpl.root.page_set {
        for area in &pages.areas {
            w.take(1)?;
            let path = area.name.as_ref().map(|name| vec![(name.clone(), 0)]).unwrap_or_default();
            w.children(&area.items, &path, 1, true)?;
        }
    }
    for req in &w.required {
        let count = w.counts.get(&(req.parent.clone(), req.name.clone())).copied().unwrap_or(0);
        if crate::layout::instance_count(&req.occur, Some(count)) != req.initial {
            return Err(XfaError::Malformed(format!(
                "The initial XFA instances of '{}' share an ambiguous data scope; preserving them would change the form's instance counts",
                req.name
            )));
        }
    }
    // The writer may report partial writes as warnings. Neither those nor an error may
    // publish a partially materialized initial structure to the caller's document.
    let mut staged = doc.clone();
    let report = write_data_ops(&mut staged, &w.ops, None)?;
    if !report.written && !w.ops.is_empty() {
        return Err(XfaError::Malformed("The initial XFA instances could not be written to the datasets packet".into()));
    }
    if !report.warnings.is_empty() {
        return Err(XfaError::Malformed(format!("The initial XFA instances could not be preserved: {}", report.warnings.join("; "))));
    }
    *doc = staged;
    Ok(report)
}

/// Examine template declarations without expanding repeated instances or allocating paths.
struct InitialNeed {
    visited: usize,
}

impl InitialNeed {
    fn take(&mut self, depth: usize) -> Result<(), XfaError> {
        self.visited = self.visited.saturating_add(1);
        if depth > super::MAX_DEPTH || self.visited > crate::MAX_FORM_NODES {
            return Err(XfaError::TooLarge("The initial XFA template exceeds the node or depth limit".into()));
        }
        Ok(())
    }

    fn subform(&mut self, sf: &Subform, depth: usize) -> Result<bool, XfaError> {
        self.take(depth)?;
        let count = crate::layout::instance_count(&sf.occur, None);
        if sf.common.name.is_some() && count > sf.occur.min {
            return Ok(true);
        }
        if count == 0 {
            return Ok(false);
        }
        self.children(&sf.children, depth + 1)
    }

    fn children(&mut self, children: &[Node], depth: usize) -> Result<bool, XfaError> {
        for node in children {
            let needed = match node {
                Node::Subform(sf) => self.subform(sf, depth)?,
                Node::Area(area) => {
                    self.take(depth)?;
                    self.children(&area.children, depth + 1)?
                }
                _ => {
                    self.take(depth)?;
                    false
                }
            };
            if needed {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

struct RequiredCount {
    parent: DataPath,
    name: String,
    occur: Occur,
    initial: usize,
}

struct InitialGroups {
    ops: Vec<DataOp>,
    /// Largest ensured index plus one, including parent groups created implicitly by a path.
    counts: HashMap<(DataPath, String), usize>,
    required: Vec<RequiredCount>,
    visited: usize,
    plan_bytes: usize,
}

impl InitialGroups {
    fn take(&mut self, depth: usize) -> Result<(), XfaError> {
        self.visited = self.visited.saturating_add(1);
        if depth > super::MAX_DEPTH || self.visited > crate::MAX_FORM_NODES {
            return Err(XfaError::TooLarge("The initial XFA instance structure exceeds the node or depth limit".into()));
        }
        Ok(())
    }

    /// Bound retained paths and their string/vector storage before cloning them into the plan.
    fn charge(&mut self, parent: &DataPath, name: &str) -> Result<(), XfaError> {
        let bytes =
            parent.iter().fold(0usize, |bytes, (part, _)| bytes.saturating_add(part.len()).saturating_add(std::mem::size_of::<(String, usize)>()));
        self.plan_bytes = self.plan_bytes.saturating_add(bytes).saturating_add(name.len()).saturating_add(std::mem::size_of::<RequiredCount>());
        if self.plan_bytes > super::write::MAX_PACKET_WRITE {
            return Err(XfaError::TooLarge("The initial XFA instance plan exceeds the data byte limit".into()));
        }
        Ok(())
    }

    /// Include implicit ancestor groups in the preflight counts as the XML writer does.
    fn ensure_path(&mut self, path: &DataPath) -> Result<(), XfaError> {
        let mut parent = Vec::new();
        for (name, index) in path {
            if super::build::xml_name(name) != *name {
                return Err(XfaError::Malformed("An initial XFA group name cannot be preserved by the data writer".into()));
            }
            if *index >= super::build::MAX_INDEX {
                return Err(XfaError::TooLarge("The initial XFA instance index exceeds the data writer's limit".into()));
            }
            self.charge(&parent, name)?;
            let count = self.counts.entry((parent.clone(), name.clone())).or_default();
            *count = (*count).max(index.saturating_add(1));
            parent.push((name.clone(), *index));
        }
        Ok(())
    }

    fn subform(&mut self, sf: &Subform, parent: &DataPath, sibling: usize, depth: usize, master: bool) -> Result<(), XfaError> {
        // Even a zero-instance declaration spends work; a wide dormant template is bounded.
        self.take(depth)?;
        let count = crate::layout::instance_count(&sf.occur, None);
        if let Some(name) = &sf.common.name {
            self.charge(parent, name)?;
            self.required.push(RequiredCount { parent: parent.clone(), name: name.clone(), occur: sf.occur, initial: count });
            if count > 0 {
                self.charge(parent, name)?;
                self.ops.push(DataOp::Instances { parent: parent.clone(), name: name.clone(), count: sibling.saturating_add(count) });
                let mut last = parent.clone();
                last.push((name.clone(), sibling.saturating_add(count).saturating_sub(1)));
                self.ensure_path(&last)?;
            }
        }
        for i in 0..count {
            if i > 0 {
                self.take(depth)?;
            }
            let mut path = parent.clone();
            if let Some(name) = &sf.common.name {
                path.push((name.clone(), sibling.saturating_add(i)));
            }
            self.children(&sf.children, &path, depth + 1, master)?;
        }
        Ok(())
    }

    fn children(&mut self, nodes: &[Node], parent: &DataPath, depth: usize, master: bool) -> Result<(), XfaError> {
        let siblings = SomIndices::of(nodes);
        for (i, node) in nodes.iter().enumerate() {
            match node {
                Node::Subform(sf) => self.subform(sf, parent, siblings.at(i), depth, master)?,
                Node::Area(area) => {
                    self.take(depth)?;
                    let mut path = parent.clone();
                    // Areas below a named body root are transparent. data_path retains
                    // them outside that lookup (masters and unnamed body roots).
                    if master && let Some(name) = &area.common.name {
                        path.push((name.clone(), siblings.at(i)));
                    }
                    self.children(&area.children, &path, depth + 1, master)?;
                }
                _ => self.take(depth)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{SaveOptions, write_incremental};

    use super::*;

    /// Same-name declarations must not turn a dormant sibling into a visible row while opening.
    #[test]
    fn ambiguous_initial_groups_fail_without_changing_document_bytes() {
        let xml = r#"<template><subform name="form" layout="tb">
            <subform name="row"><occur min="0" initial="0" max="1"/></subform>
            <subform name="row"><occur min="1" initial="2" max="2"/></subform>
        </subform></template>"#;
        let mut doc = Document::open(Arc::new(crate::fixtures::shell(xml))).unwrap();
        let (tpl, _) = crate::parse(xml).unwrap();
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let error = preserve_initial_instances(&mut doc, &tpl).unwrap_err();
        assert!(error.to_string().contains("ambiguous data scope"), "{error}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
        assert!(crate::data_of(&doc).is_none());
    }

    /// Forms fully protected by minimum counts do not acquire a datasets packet just by opening.
    #[test]
    fn minimum_only_initial_groups_leave_the_source_untouched() {
        let xml = r#"<template><subform name="form" layout="tb">
            <subform name="row"><occur min="0" initial="0" max="1"/></subform>
            <subform name="row"><occur min="1" max="1"/></subform>
        </subform></template>"#;
        let mut doc = Document::open(Arc::new(crate::fixtures::shell(xml))).unwrap();
        let (tpl, _) = crate::parse(xml).unwrap();
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert_eq!(preserve_initial_instances(&mut doc, &tpl).unwrap(), DatasetsWrite::default());
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
        assert!(crate::data_of(&doc).is_none());
    }

    /// Names the writer would sanitize must not silently change a group binding.
    #[test]
    fn unwritable_initial_group_leaves_the_source_untouched() {
        let xml = r#"<template><subform name="form"><subform name="bad name">
            <occur min="0" initial="2" max="2"/>
        </subform></subform></template>"#;
        let mut doc = Document::open(Arc::new(crate::fixtures::shell(xml))).unwrap();
        let (tpl, _) = crate::parse(xml).unwrap();
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(preserve_initial_instances(&mut doc, &tpl).is_err());
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
    }
    /// The writer's node cap may produce partial output with a warning: discard it atomically.
    #[test]
    fn capped_initial_group_write_leaves_the_source_untouched() {
        let xml = r#"<template><subform name="form"><subform name="outer">
            <occur min="0" initial="21" max="21"/>
            <subform name="middle"><occur min="0" initial="50" max="50"/>
                <subform name="inner"><occur min="0" initial="20" max="20"/></subform>
            </subform>
        </subform></subform></template>"#;
        let mut doc = Document::open(Arc::new(crate::fixtures::shell(xml))).unwrap();
        let (tpl, _) = crate::parse(xml).unwrap();
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let error = preserve_initial_instances(&mut doc, &tpl).unwrap_err();
        assert!(error.to_string().contains("could not be preserved"), "{error}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
    }

    /// A master named like the body root has no unambiguous mapping in the current resolver.
    #[test]
    fn colliding_master_and_root_names_fail_without_changing_source() {
        let xml = r#"<template><subform name="form" layout="tb">
            <pageSet><pageArea name="form">
                <area name="header"><subform name="rep"><occur min="1" initial="2" max="2"/></subform></area>
            </pageArea></pageSet>
            <area name="header"><field name="k"/></area>
        </subform></template>"#;
        let mut doc = Document::open(Arc::new(crate::fixtures::shell(xml))).unwrap();
        let (tpl, _) = crate::parse(xml).unwrap();
        let before = write_incremental(&doc, &SaveOptions::default()).unwrap();
        let error = preserve_initial_instances(&mut doc, &tpl).unwrap_err();
        assert!(error.to_string().contains("ambiguous shared body-root and page-master name"), "{error}");
        assert_eq!(write_incremental(&doc, &SaveOptions::default()).unwrap(), before);
        assert!(crate::data_of(&doc).is_none());
        // The same names without a count needing preservation still take the no-op path.
        let xml = xml.replace("initial=\"2\"", "initial=\"1\"");
        let (tpl, _) = crate::parse(&xml).unwrap();
        assert_eq!(preserve_initial_instances(&mut doc, &tpl).unwrap(), DatasetsWrite::default());
    }
}
