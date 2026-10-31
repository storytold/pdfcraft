//! Prototypes (XFA 3.3, chapter 6, "Prototypes"): an element with `use` or `usehref` inherits
//! the attributes, data content and children of the element it names, and overrides what it
//! declares itself. Designer's style sheets work this way: a field names its style, and its
//! size, border, font and caption placement are the style's.
//!
//! [`expand`] writes the template again with every reference resolved, so the parser reads
//! plain elements. Prototypes are resolved once, when the template is loaded, as the
//! specification says ("Prototypes", the note before "Defining Prototypes").
//!
//! Rules ("Overriding Prototype Properties"): every attribute but `id`, `name`, `use` and
//! `usehref` is inherited unless the element has its own; the prototype's data content is
//! inherited unless the element has non-empty content of its own; children are matched by
//! class and then by position among that class, and a matched prototype child is in turn the
//! prototype of the element's child. A `ui` or `value` holds one alternative (one widget, one
//! kind of content), so the nearest layer's choice is kept and only that one is merged; rich
//! text (`exData`) is content and is taken whole from the nearest layer that has any.
//!
//! Only references into the same template are followed: `usehref` with no document (or the
//! legacy `.`), as `#id` or `#som(expression)`, and `use` as `#id` or a SOM expression. A
//! reference to another document, to nothing, or back into itself is left out, with a warning.
//! The SOM subset is the one Designer writes: `$template.#subform.styles.Heading[1]`, each
//! step a name or a `#class` with an optional index, looking through `proto` elements and
//! nameless subforms.

use std::collections::{HashMap, HashSet};

use roxmltree::{Node, NodeId};

/// Deepest element nesting written. A prototype may contain a reference to an element that
/// contains it (Example 6.8): such loops are cut, and this bounds what remains.
const MAX_DEPTH: usize = 256;
/// Most layers (an element and the prototypes it inherits from) merged into one element.
const MAX_LAYERS: usize = 32;
/// Most bytes written: at least `MIN_OUTPUT`, at most `GROWTH` times the template and never
/// more than `MAX_OUTPUT`. Each reference repeats its prototype, so a hostile template could
/// otherwise grow without bound.
const MAX_OUTPUT: usize = 64 << 20;
const MIN_OUTPUT: usize = 1 << 20;
const GROWTH: usize = 4;
/// Most elements looked at to resolve all SOM references together.
const MAX_SEARCH: usize = 2_000_000;
/// Most children, attributes and namespaces looked at to merge all the layers together. Each
/// reference looks through its prototype's children again, also those that write nothing
/// (`proto`, and the `ui` and `value` alternatives not chosen), so the output budget alone does
/// not bound this. Real forms cost one unit for every 5.5 to 36 bytes written (the largest
/// measured, a 14 MB template, costs 2.5 million), so writing as much as the 64 MB output
/// budget allows costs some 12 million: this keeps a margin of five over that, and stops a
/// hostile template after about a second.
const MAX_WORK: usize = 64_000_000;
/// The namespace the `xml` prefix is always bound to.
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// The template written again with its prototype references resolved (`None` when it has
/// none, or when resolving them would pass a budget: the template is then read as it is), and
/// the warnings for what was left out.
pub(crate) fn expand(template: Node) -> (Option<String>, Vec<String>) {
    let referencing = |n: &Node| {
        n.is_element() && (n.attribute("use").is_some_and(|u| !u.trim().is_empty()) || n.attribute("usehref").is_some_and(|u| !u.trim().is_empty()))
    };
    if !template.descendants().any(|n| referencing(&n)) {
        return (None, Vec::new());
    }
    let limit = template.range().len().saturating_mul(GROWTH).clamp(MIN_OUTPUT, MAX_OUTPUT);
    let mut w = Writer {
        template,
        out: String::new(),
        limit,
        failed: None,
        ids: None,
        soms: HashMap::new(),
        search: MAX_SEARCH,
        work: MAX_WORK,
        active: HashMap::new(),
        missing: false,
        external: false,
        looped: false,
        layers_cut: false,
    };
    w.element(&[template], &Scope::new(), 0);
    let mut warnings = Vec::new();
    if w.external {
        warnings.push("a prototype reference (usehref) points to another document; it is not followed".into());
    }
    if w.missing {
        warnings.push("a prototype reference (use or usehref) names nothing of its kind in the template; the element is read without it".into());
    }
    if w.looped {
        warnings.push("a prototype reference leads back to itself; the loop is cut there".into());
    }
    if w.layers_cut {
        warnings.push(format!("a chain of prototypes is longer than {MAX_LAYERS}; the farthest are left out"));
    }
    match w.failed {
        Some(why) => {
            warnings.push(format!("the template's prototypes {why}; it is read without resolving them"));
            (None, warnings)
        }
        None => (Some(w.out), warnings),
    }
}

/// Namespace bindings in scope where an element is written: URI by prefix (`None` for the
/// default namespace).
type Scope<'a> = HashMap<Option<&'a str>, &'a str>;

struct Writer<'a, 'd> {
    template: Node<'a, 'd>,
    out: String,
    limit: usize,
    /// Why expanding stopped, for the warning.
    failed: Option<String>,
    /// The template's elements by `id`, built on the first `#id` reference.
    ids: Option<HashMap<&'a str, Node<'a, 'd>>>,
    /// SOM references already resolved, by their text.
    soms: HashMap<&'a str, Option<Node<'a, 'd>>>,
    /// Elements left to look at for SOM references.
    search: usize,
    /// Children, attributes and namespaces left to look at while merging layers.
    work: usize,
    /// The elements being written around the current one: inheriting from one is a loop.
    active: HashMap<NodeId, u32>,
    missing: bool,
    external: bool,
    looped: bool,
    layers_cut: bool,
}

fn escape(s: &str, attribute: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' if attribute => out.push_str("&#13;"),
            '\t' if attribute => out.push_str("&#9;"),
            _ => out.push(c),
        }
    }
}

/// Containers a SOM step looks through: they have no name of their own.
fn transparent(n: Node) -> bool {
    n.tag_name().name() == "proto" || n.tag_name().name() == "subform" && n.attribute("name").is_none_or(str::is_empty)
}

/// Does `n` have content of its own: non-blank text, or (`elements`) any child element?
fn has_content(n: &Node, elements: bool) -> bool {
    n.children().any(|c| elements && c.is_element() || c.is_text() && c.text().is_some_and(|t| !t.trim().is_empty()))
}

/// In a `ui` or a `value`, is `c` one of the alternatives of which only one is meant?
fn alternative(tag: &str, c: &Node) -> bool {
    c.is_element() && (tag == "value" || tag == "ui" && !matches!(c.tag_name().name(), "picture" | "extras"))
}

impl<'a, 'd> Writer<'a, 'd> {
    /// The element `n` names as its prototype. `usehref` wins over `use` when both are set.
    fn reference(&mut self, n: Node<'a, 'd>) -> Option<Node<'a, 'd>> {
        let set = |name: &str| n.attribute(name).map(str::trim).filter(|v| !v.is_empty());
        let found = if let Some(href) = set("usehref") {
            match href.split_once('#') {
                Some((doc, fragment)) if matches!(doc.trim(), "" | ".") => {
                    let fragment = fragment.trim();
                    match fragment.strip_prefix("som(").and_then(|s| s.strip_suffix(')')) {
                        Some(expr) => self.som(expr.trim()),
                        None => self.by_id(fragment),
                    }
                }
                _ => {
                    self.external = true;
                    return None;
                }
            }
        } else {
            let r = set("use")?;
            match r.strip_prefix('#') {
                Some(id) => self.by_id(id.trim()),
                // A SOM expression; the specification's own Example 6.5 writes a bare id.
                None => self.som(r).or_else(|| self.by_id(r)),
            }
        };
        match found {
            // A budget ran out while looking: the template is read unresolved, with that warning.
            _ if self.failed.is_some() => None,
            Some(t) if t == n => {
                self.looped = true;
                None
            }
            Some(t) if t.tag_name().name() == n.tag_name().name() => Some(t),
            _ => {
                self.missing = true;
                None
            }
        }
    }

    fn by_id(&mut self, id: &str) -> Option<Node<'a, 'd>> {
        let template = self.template;
        let ids = self.ids.get_or_insert_with(|| {
            let mut ids = HashMap::new();
            for n in template.descendants().filter(Node::is_element) {
                if let Some(id) = n.attribute("id") {
                    ids.entry(id).or_insert(n);
                }
            }
            ids
        });
        ids.get(id).copied()
    }

    /// A SOM expression from the template down, such as `$template.#subform.styles.Heading[1]`.
    fn som(&mut self, expr: &'a str) -> Option<Node<'a, 'd>> {
        if let Some(found) = self.soms.get(expr) {
            return *found;
        }
        let found = self.resolve(expr);
        // A look-up the budget cut short has not found nothing: it is not cached, and the
        // template is read unresolved (`step` set `failed`).
        if self.failed.is_none() {
            self.soms.insert(expr, found);
        }
        found
    }

    fn resolve(&mut self, expr: &str) -> Option<Node<'a, 'd>> {
        let mut steps = expr.split('.').map(str::trim).peekable();
        while steps.peek().is_some_and(|s| matches!(*s, "$template" | "template" | "template[0]" | "xfa" | "$xfa" | "xfa[0]")) {
            steps.next();
        }
        let mut at = self.template;
        for step in steps {
            let (name, index) = match step.split_once('[') {
                Some((name, index)) => (name.trim(), index.strip_suffix(']')?.trim().parse::<usize>().ok()?),
                None => (step, 0),
            };
            if name.is_empty() || name == "#" {
                return None;
            }
            let mut matches = Vec::new();
            self.step(at, name, index, &mut matches, 0);
            at = *matches.get(index)?;
        }
        (at != self.template).then_some(at)
    }

    /// The children of `at` called `name` (or of class `#name`), looking through nameless
    /// containers, until the one at `index` is found.
    fn step(&mut self, at: Node<'a, 'd>, name: &str, index: usize, out: &mut Vec<Node<'a, 'd>>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for c in at.children().filter(Node::is_element) {
            if out.len() > index || self.failed.is_some() {
                return;
            }
            if self.search == 0 {
                self.failed = Some(format!("need more than {MAX_SEARCH} elements looked at to resolve their SOM references"));
                return;
            }
            self.search -= 1;
            let hit = match name.strip_prefix('#') {
                Some(class) => c.tag_name().name() == class,
                None => c.attribute("name") == Some(name),
            };
            if hit {
                out.push(c);
            } else if transparent(c) {
                self.step(c, name, index, out, depth + 1);
            }
        }
    }

    /// `n` and the prototypes it inherits from, nearest first.
    fn chain(&mut self, n: Node<'a, 'd>, layers: &mut Vec<Node<'a, 'd>>) {
        let mut seen = HashSet::from([n.id()]);
        layers.push(n);
        let mut at = n;
        while let Some(t) = self.reference(at) {
            if !seen.insert(t.id()) || self.active.contains_key(&t.id()) {
                self.looped = true;
                return;
            }
            if layers.len() >= MAX_LAYERS {
                self.layers_cut = true;
                return;
            }
            layers.push(t);
            at = t;
        }
    }

    /// Take `n` from the work budget; past it, expanding stops.
    fn charge(&mut self, n: usize) -> bool {
        match self.work.checked_sub(n) {
            Some(left) => {
                self.work = left;
                true
            }
            None => {
                self.work = 0;
                self.failed = Some(format!("need more than {MAX_WORK} children, attributes and namespaces looked at to merge them"));
                false
            }
        }
    }

    /// Write the namespace bindings `layers` have that differ from those in `scope` around their
    /// element, by prefix, and return the scope inside it: the nearest layer's binding of a prefix
    /// wins, and a prefix bound around the element to another URI is bound again here (an XHTML
    /// `body` inside a template whose default namespace is XFA's).
    fn namespaces(&mut self, layers: &[Node<'a, 'd>], scope: &Scope<'a>) -> Scope<'a> {
        let mut inner: Scope<'a> = scope.clone();
        let mut decided: HashSet<Option<&'a str>> = HashSet::new();
        for layer in layers {
            for ns in layer.namespaces() {
                let (name, uri) = (ns.name(), ns.uri());
                if name == Some("xml") || !decided.insert(name) || scope.get(&name) == Some(&uri) {
                    continue;
                }
                match name {
                    Some(name) => {
                        self.out.push_str(" xmlns:");
                        self.out.push_str(name);
                    }
                    None => self.out.push_str(" xmlns"),
                }
                self.out.push_str("=\"");
                escape(uri, true, &mut self.out);
                self.out.push('"');
                inner.insert(name, uri);
            }
        }
        inner
    }

    /// Write the attributes of the element `layers` make, inside the namespace scope `inner`.
    fn attributes(&mut self, layers: &[Node<'a, 'd>], inner: &Scope<'a>) {
        // Attributes: the nearest layer that has one wins; `id` and `name` are not inherited.
        // Attributes are told apart by namespace URI and local name. One whose layer's prefix is
        // bound here to another URI (the nearest layer's binding won) is written with a fresh
        // prefix, declared on this element.
        let mut written: HashSet<(Option<&str>, &str)> = HashSet::new();
        let mut fresh: HashMap<&str, String> = HashMap::new();
        let mut next = 0usize;
        for (i, layer) in layers.iter().enumerate() {
            for a in layer.attributes() {
                let inherited = i > 0 && matches!(a.name(), "id" | "name");
                let reference = a.namespace().is_none() && matches!(a.name(), "use" | "usehref");
                if inherited || reference || !written.insert((a.namespace(), a.name())) {
                    continue;
                }
                let name = match a.namespace() {
                    None => a.name().to_string(),
                    Some(XML_NS) => format!("xml:{}", a.name()),
                    Some(uri) => match layer.lookup_prefix(uri).filter(|p| !p.is_empty()) {
                        Some(p) if inner.get(&Some(p)) == Some(&uri) => format!("{p}:{}", a.name()),
                        _ => {
                            let p = match fresh.get(uri) {
                                Some(p) => p.clone(),
                                None => {
                                    // `next` only grows, and skips the prefixes in scope here,
                                    // so it passes at most `inner.len()` of them.
                                    let p = loop {
                                        let p = format!("ns{next}");
                                        next = next.saturating_add(1);
                                        if !inner.contains_key(&Some(p.as_str())) {
                                            break p;
                                        }
                                    };
                                    self.out.push_str(" xmlns:");
                                    self.out.push_str(&p);
                                    self.out.push_str("=\"");
                                    escape(uri, true, &mut self.out);
                                    self.out.push('"');
                                    fresh.insert(uri, p.clone());
                                    p
                                }
                            };
                            format!("{p}:{}", a.name())
                        }
                    },
                };
                self.out.push(' ');
                self.out.push_str(&name);
                self.out.push_str("=\"");
                escape(a.value(), true, &mut self.out);
                self.out.push('"');
            }
        }
    }

    /// Write the element that `stack` makes together: `stack[0]` over `stack[1]` over …, each
    /// an element of the same class. `scope` holds the namespaces already declared around it.
    fn element(&mut self, stack: &[Node<'a, 'd>], scope: &Scope<'a>, depth: usize) {
        if self.failed.is_some() {
            return;
        }
        if depth > MAX_DEPTH {
            self.failed = Some(format!("nest deeper than {MAX_DEPTH} elements"));
            return;
        }
        if self.out.len() > self.limit {
            self.failed = Some(format!("would make it larger than {} MB", self.limit >> 20));
            return;
        }
        // Every layer may itself inherit from a prototype.
        let mut layers = Vec::new();
        for l in stack {
            self.chain(*l, &mut layers);
            if layers.len() >= MAX_LAYERS {
                break;
            }
        }
        // Charge what merging looks at before looking: each layer's children (counted no
        // further than the budget left), attributes and namespaces, and the scope around it.
        let mut cost = scope.len().saturating_add(layers.len());
        for l in &layers {
            let children = l.children().take(self.work.saturating_add(1)).count();
            cost = cost.saturating_add(children).saturating_add(l.attributes().len()).saturating_add(l.namespaces().len());
        }
        if self.failed.is_some() || !self.charge(cost) {
            return;
        }
        let Some(&top) = layers.first() else { return };
        for l in &layers {
            *self.active.entry(l.id()).or_insert(0) += 1;
        }
        let tag = top.tag_name().name();
        let prefix =
            |n: Node, ns: Option<&str>| ns.and_then(|ns| n.lookup_prefix(ns)).filter(|p| !p.is_empty()).map(|p| format!("{p}:")).unwrap_or_default();
        let qname = format!("{}{tag}", prefix(top, top.tag_name().namespace()));
        self.out.push('<');
        self.out.push_str(&qname);
        let mut inner = self.namespaces(&layers, scope);
        // An element in no namespace inside a default namespace undeclares it.
        if top.tag_name().namespace().is_none() && inner.get(&None).is_some_and(|u| !u.is_empty()) {
            self.out.push_str(" xmlns=\"\"");
            inner.insert(None, "");
        }
        self.attributes(&layers, &inner);
        self.out.push('>');
        // Rich text is taken whole from the nearest layer that has any.
        let cut = if tag == "exData" { layers.iter().position(|l| has_content(l, true)).map_or(layers.len(), |i| i + 1) } else { layers.len() };
        let merged = layers.get(..cut).unwrap_or(&layers);
        // Data content: the nearest layer's that has any (empty content overrides nothing).
        let text_layer = merged.iter().position(|l| has_content(l, false)).unwrap_or(0);
        let chosen = if matches!(tag, "ui" | "value") {
            merged.iter().find_map(|l| l.children().find(|c| alternative(tag, c)).map(|c| c.tag_name().name()))
        } else {
            None
        };
        // Prototypes take no part in the form; of the alternatives one is kept. A class left
        // out is left out whole, so it need not be counted either.
        let left_out = |c: &Node| c.tag_name().name() == "proto" || alternative(tag, c) && chosen != Some(c.tag_name().name());
        // The deeper layers' children by (class, position among that class).
        let index = |l: &Node<'a, 'd>| {
            let mut counts: HashMap<&str, usize> = HashMap::new();
            let mut by: HashMap<(&'a str, usize), Node<'a, 'd>> = HashMap::new();
            for c in l.children().filter(|c| c.is_element() && !left_out(c)) {
                let k = counts.entry(c.tag_name().name()).or_insert(0);
                by.insert((c.tag_name().name(), *k), c);
                *k += 1;
            }
            by
        };
        let below: Vec<_> = merged.iter().skip(1).map(index).collect();
        let mut done: HashSet<(&str, usize)> = HashSet::new();
        for (i, layer) in merged.iter().enumerate() {
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for c in layer.children() {
                if c.is_text() && i == text_layer {
                    escape(c.text().unwrap_or_default(), false, &mut self.out);
                }
                if !c.is_element() || left_out(&c) {
                    continue;
                }
                let ctag = c.tag_name().name();
                let n = {
                    let k = counts.entry(ctag).or_insert(0);
                    *k += 1;
                    *k - 1
                };
                if !done.insert((ctag, n)) {
                    continue;
                }
                let stack: Vec<Node<'a, 'd>> = std::iter::once(c).chain(below.iter().skip(i).filter_map(|b| b.get(&(ctag, n)).copied())).collect();
                self.element(&stack, &inner, depth + 1);
            }
        }
        self.out.push_str("</");
        self.out.push_str(&qname);
        self.out.push('>');
        for l in &layers {
            if let Some(k) = self.active.get_mut(&l.id()) {
                *k = k.saturating_sub(1);
                if *k == 0 {
                    self.active.remove(&l.id());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
