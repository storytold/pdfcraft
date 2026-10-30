#[cfg(not(target_arch = "wasm32"))]
use super::CLOCK_EVERY;
use super::ast::{Expr, Idx};
use super::value::{Val, num_text, parse_num};
use super::{
    HNode, HashSet, Interp, LIST_TOO_LONG, MAX_LIST, MAX_PARSE_DEPTH, MAX_STEPS, R, RAN_TOO_LONG, child_named, descendants_named, instances,
    resolve_som,
};

impl Interp<'_> {
    pub(super) fn step(&mut self) -> R<()> {
        self.charge(1)
    }

    /// `n` units of work done (a step per statement or expression, more for work on long
    /// strings and lists).
    pub(super) fn charge(&mut self, n: u64) -> R<()> {
        let before = self.steps;
        self.steps = self.steps.saturating_add(n);
        if self.steps > MAX_STEPS {
            return Err(RAN_TOO_LONG.into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.steps / CLOCK_EVERY != before / CLOCK_EVERY
            && let Some(d) = self.deadline
            && std::time::Instant::now() > d
        {
            return Err(RAN_TOO_LONG.into());
        }
        #[cfg(target_arch = "wasm32")]
        let _ = before;
        Ok(())
    }

    /// `more` nodes into `out`, each once, never past [`MAX_LIST`].
    pub(super) fn gather(&mut self, out: &mut Vec<usize>, seen: &mut HashSet<usize>, more: impl IntoIterator<Item = usize>) -> R<()> {
        for i in more {
            if seen.insert(i) {
                out.push(i);
                if out.len() > MAX_LIST {
                    return Err(LIST_TOO_LONG.into());
                }
            }
        }
        self.charge(1)
    }

    /// The nodes of `v`, each once.
    pub(super) fn nodes_list(&self, v: &Val) -> Vec<usize> {
        let mut seen = HashSet::new();
        self.nodes_of(v).into_iter().filter(|i| seen.insert(*i)).collect()
    }

    pub(super) fn node(&self, i: usize) -> Option<&HNode> {
        self.h.nodes.get(i).filter(|n| !n.gone)
    }

    // ── coercions

    /// A node's value as a FormCalc value.
    fn node_value(&self, i: usize) -> Val {
        let Some(n) = self.node(i) else { return Val::Null };
        if n.value.is_empty() {
            return Val::Null;
        }
        if n.numeric
            && let Some(v) = parse_num(&n.value)
        {
            return Val::Num(v);
        }
        Val::Str(n.value.clone())
    }

    /// What copying `v` (or the values it stands for) costs, in steps: one per 256 bytes, so
    /// copying long strings over and over runs out of steps even where there is no clock
    /// (wasm). A form object counts its value; a list its items too.
    pub(super) fn cost(&self, v: &Val) -> u64 {
        let mut bytes = 0usize;
        let mut todo = vec![v];
        while let Some(v) = todo.pop() {
            match v {
                Val::Str(s) => bytes = bytes.saturating_add(s.len()),
                Val::Node(i) => bytes = bytes.saturating_add(self.node(*i).map_or(0, |n| n.value.len())),
                Val::List(items) => {
                    bytes = bytes.saturating_add(items.len().saturating_mul(16));
                    todo.extend(items.iter());
                }
                _ => {}
            }
        }
        (bytes / 256) as u64
    }

    /// [`Self::deref`], charged for the copy.
    pub(super) fn take(&mut self, v: Val) -> R<Val> {
        self.charge(self.cost(&v))?;
        Ok(self.deref(v))
    }

    /// Objects become their values; lists of objects become lists of values.
    pub(super) fn deref(&self, v: Val) -> Val {
        match v {
            Val::Node(i) => self.node_value(i),
            Val::List(items) => Val::List(items.into_iter().map(|x| self.deref(x)).collect()),
            other => other,
        }
    }

    pub(super) fn to_num(&self, v: &Val) -> f64 {
        match v {
            Val::Null => 0.0,
            Val::Num(n) => *n,
            Val::Str(s) => parse_num(s).unwrap_or(0.0),
            Val::Node(i) => self.to_num(&self.node_value(*i)),
            Val::List(items) => items.first().map_or(0.0, |x| self.to_num(x)),
            _ => 0.0,
        }
    }

    pub(super) fn to_str(&self, v: &Val) -> String {
        match v {
            Val::Null => String::new(),
            Val::Num(n) => num_text(*n),
            Val::Str(s) => s.clone(),
            Val::Node(i) => self.to_str(&self.node_value(*i)),
            Val::List(items) => items.first().map_or(String::new(), |x| self.to_str(x)),
            Val::Host => "host".into(),
            Val::Layout => "layout".into(),
            Val::Event => "event".into(),
            Val::Xfa => "xfa".into(),
            Val::Im(..) => "instanceManager".into(),
            Val::Sink => String::new(),
        }
    }

    pub(super) fn truthy(&self, v: &Val) -> bool {
        match v {
            Val::Null => false,
            Val::Num(n) => *n != 0.0 && !n.is_nan(),
            Val::Str(s) => parse_num(s).is_some_and(|n| n != 0.0),
            Val::Node(i) => {
                let d = self.node_value(*i);
                self.truthy(&d)
            }
            Val::List(items) => !items.is_empty(),
            _ => true,
        }
    }

    pub(super) fn is_null(&self, v: &Val) -> bool {
        match v {
            Val::Null => true,
            Val::Node(i) => self.node(*i).is_none_or(|n| n.value.is_empty()),
            Val::List(items) => items.is_empty(),
            _ => false,
        }
    }

    // ── scopes

    fn lookup(&self, name: &str) -> Option<Val> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    pub(super) fn set_var(&mut self, name: &str, v: Val) -> bool {
        for s in self.scopes.iter_mut().rev() {
            if let Some(slot) = s.get_mut(name) {
                *slot = v;
                return true;
            }
        }
        false
    }

    pub(super) fn declare(&mut self, name: &str, v: Val) {
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name.to_string(), v);
        }
    }

    // ── accessors

    /// The objects an identifier names from the current node: `$`, the special objects, an
    /// instance manager (`_name`), a variable, or a form object by name (children first, then
    /// the containing subforms).
    pub(super) fn ident(&self, name: &str) -> R<Val> {
        if let Some(v) = self.lookup(name) {
            return Ok(v);
        }
        let lower = name.to_ascii_lowercase();
        Ok(match lower.as_str() {
            "$" | "this" => Val::Node(self.h.current),
            "$host" => Val::Host,
            "$layout" => Val::Layout,
            "$event" => Val::Event,
            "$form" | "$data" | "$dataset" | "$datasets" => Val::Node(0),
            "$record" => Val::Node(if self.node(1).is_some() { 1 } else { 0 }),
            "$template" | "$xfa" => Val::Xfa,
            "xfa" => Val::Xfa,
            _ => {
                if let Some(child) = name.strip_prefix('_') {
                    // The instance manager of `child`, looked for like a name.
                    let found = resolve_som(self.h, self.h.current, child);
                    let Some(&first) = found.first() else { return Err(format!("unknown instance manager _{child}")) };
                    let parent = self.node(first).and_then(|n| n.parent).ok_or_else(|| format!("unknown instance manager _{child}"))?;
                    return Ok(Val::Im(parent, child.to_string()));
                }
                let found = resolve_som(self.h, self.h.current, name);
                match found.as_slice() {
                    [] => return Err(format!("unknown name {name}")),
                    [one] => Val::Node(*one),
                    many => Val::List(many.iter().map(|i| Val::Node(*i)).collect()),
                }
            }
        })
    }

    /// Every object a value stands for.
    pub(super) fn nodes_of(&self, v: &Val) -> Vec<usize> {
        match v {
            Val::Node(i) => vec![*i],
            Val::List(items) => items.iter().flat_map(|x| self.nodes_of(x)).collect(),
            _ => Vec::new(),
        }
    }

    pub(super) fn member(&mut self, base: Val, name: &str) -> R<Val> {
        match base {
            Val::Node(i) => self.node_member(i, name),
            Val::List(items) => {
                let mut out = Vec::new();
                let mut seen = HashSet::new();
                for it in items {
                    match self.member(it, name)? {
                        Val::List(more) => {
                            for m in more {
                                self.list_push(&mut out, &mut seen, m)?;
                            }
                        }
                        Val::Null => {}
                        v => self.list_push(&mut out, &mut seen, v)?,
                    }
                }
                Ok(match out.len() {
                    0 => Val::Null,
                    1 => out.remove(0),
                    _ => Val::List(out),
                })
            }
            Val::Host => Ok(match name.to_ascii_lowercase().as_str() {
                "numpages" => Val::Num(self.h.doc.page_count as f64),
                "currentpage" => Val::Num(self.h.doc.page as f64),
                "name" => Val::Str("Acrobat".into()),
                "apptype" => Val::Str("Exchange-Pro".into()),
                "version" => Val::Str("11".into()),
                "variation" => Val::Str("Full".into()),
                "language" => Val::Str("en".into()),
                "platform" => Val::Str(crate::platform().into()),
                "title" => Val::Str(self.h.doc.file_name.clone()),
                "calculationsenabled" | "validationsenabled" => Val::Num(1.0),
                _ => Val::Sink,
            }),
            Val::Layout => Ok(Val::Sink),
            Val::Event => Ok(match name.to_ascii_lowercase().as_str() {
                "name" => Val::Str(self.event.activity.clone()),
                "newtext" | "fulltext" => Val::Str(self.event.new_text.clone()),
                "prevtext" => Val::Str(self.event.prev_text.clone()),
                "change" => Val::Str(String::new()),
                "target" => Val::Node(self.h.current),
                "selstart" | "selend" | "cancelaction" | "reenter" | "shift" | "modifier" | "keydown" | "commitkey" => Val::Num(0.0),
                _ => Val::Sink,
            }),
            Val::Xfa => Ok(match name.to_ascii_lowercase().as_str() {
                "form" | "datasets" | "data" => Val::Node(0),
                "record" => Val::Node(if self.node(1).is_some() { 1 } else { 0 }),
                "host" => Val::Host,
                "layout" => Val::Layout,
                "event" => Val::Event,
                _ => Val::Sink,
            }),
            Val::Im(parent, child) => Ok(match name.to_ascii_lowercase().as_str() {
                "count" => Val::Num(instances(self.h, parent, &child).len() as f64),
                "min" => Val::Num(instances(self.h, parent, &child).first().and_then(|i| self.node(*i)).map_or(0.0, |n| n.occur_min as f64)),
                "max" => Val::Num(
                    instances(self.h, parent, &child).first().and_then(|i| self.node(*i)).and_then(|n| n.occur_max).map_or(-1.0, |m| m as f64),
                ),
                "classname" => Val::Str("instanceManager".into()),
                _ => Val::Sink,
            }),
            Val::Sink => Ok(Val::Sink),
            Val::Null => Err(format!("no object to take {name} from")),
            Val::Num(_) | Val::Str(_) => Err(format!("{} has no property {name}", self.to_str(&base))),
        }
    }

    /// `v` onto `out`: a node only once, never past [`MAX_LIST`].
    fn list_push(&mut self, out: &mut Vec<Val>, seen: &mut HashSet<usize>, v: Val) -> R<()> {
        if let Val::Node(i) = v
            && !seen.insert(i)
        {
            return Ok(());
        }
        out.push(v);
        if out.len() > MAX_LIST {
            return Err(LIST_TOO_LONG.into());
        }
        self.charge(1)
    }

    /// Property `name` of form object `i` as FormCalc reads it: its value, attributes,
    /// relatives and children by name.
    fn node_member(&mut self, i: usize, name: &str) -> R<Val> {
        let Some(n) = self.node(i) else { return Ok(Val::Null) };
        Ok(match name {
            "formattedValue" => match &n.formatted {
                Some(f) => Val::Str(f.clone()),
                None => self.node_value(i),
            },
            "rawValue" | "value" => self.node_value(i),
            "name" => Val::Str(n.name.clone()),
            "somExpression" => Val::Str(n.som.clone()),
            "className" => Val::Str(n.kind.class_name().into()),
            "index" | "instanceIndex" => Val::Num(n.index as f64),
            "presence" => Val::Str(n.presence.clone()),
            "access" => Val::Str(n.access.clone()),
            "isNull" => Val::Num(if n.value.is_empty() { 1.0 } else { 0.0 }),
            "mandatory" => Val::Str("disabled".into()),
            "parent" => n.parent.map_or(Val::Null, Val::Node),
            "nodes" => Val::List(n.children.iter().copied().filter(|c| self.node(*c).is_some()).map(Val::Node).collect()),
            "all" => match n.parent {
                Some(p) => Val::List(instances(self.h, p, &n.name).into_iter().map(Val::Node).collect()),
                None => Val::Node(i),
            },
            "instanceManager" => match n.parent {
                Some(p) => Val::Im(p, n.name.clone()),
                None => Val::Null,
            },
            "border" | "ui" | "font" | "caption" | "fillColor" | "borderColor" | "fontColor" | "margin" | "para" | "assist" | "validate"
            | "format" | "items" | "bind" | "keep" | "h" | "w" | "x" | "y" | "minH" | "maxH" | "minW" | "maxW" | "layout" | "model"
            | "oneOfChild" | "locale" | "relevant" | "colSpan" | "dataNode" | "defaultValue" | "editValue" | "selectedIndex" | "event"
            | "calculate" | "traversal" | "extras" | "desc" => Val::Sink,
            other => {
                if let Some(child) = other.strip_prefix('_') {
                    return Ok(if child_named(self.h, i, child).is_some() { Val::Im(i, child.to_string()) } else { Val::Null });
                }
                let found = instances(self.h, i, other);
                match found.as_slice() {
                    [] => Val::Null,
                    [one] => Val::Node(*one),
                    many => Val::List(many.iter().map(|k| Val::Node(*k)).collect()),
                }
            }
        })
    }

    /// `base[n]` / `base[*]`: all the instances the last step named, then one or all.
    pub(super) fn index(&mut self, base: &Expr, idx: &Idx) -> R<Val> {
        let all = self.all_of(base)?;
        match idx {
            Idx::All => Ok(Val::List(all.into_iter().map(Val::Node).collect())),
            Idx::Abs(e) => {
                let v = self.eval(e)?;
                let n = self.to_num(&v);
                if !n.is_finite() || n < 0.0 {
                    return Ok(Val::Null);
                }
                Ok(all.get(n as usize).map_or(Val::Null, |i| Val::Node(*i)))
            }
            Idx::Rel(k) => {
                // From the instance the script runs in (`row[-1]` in a row is the row above),
                // else from the first.
                let here = all.iter().position(|&i| self.encloses(i, self.h.current)).unwrap_or(0);
                let at = (here as i64).saturating_add(*k);
                Ok(usize::try_from(at).ok().and_then(|at| all.get(at)).map_or(Val::Null, |i| Val::Node(*i)))
            }
        }
    }

    /// Whether `node` is `inner` or one of its containers.
    fn encloses(&self, node: usize, inner: usize) -> bool {
        let mut at = Some(inner);
        let mut guard = 0;
        while let Some(i) = at
            && guard < MAX_PARSE_DEPTH
        {
            if i == node {
                return true;
            }
            at = self.h.nodes.get(i).and_then(|n| n.parent);
            guard += 1;
        }
        false
    }

    /// Every instance an accessor names (`Row` → all rows, not just the first), each once.
    pub(super) fn all_of(&mut self, e: &Expr) -> R<Vec<usize>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        match e {
            Expr::Ident(name) => {
                if self.lookup(name).is_some() || name.starts_with('$') || name.eq_ignore_ascii_case("xfa") {
                    let v = self.eval(e)?;
                    return Ok(self.nodes_list(&v));
                }
                // The name resolves to its first instance; `row[*]` means every sibling of it.
                for first in resolve_som(self.h, self.h.current, name) {
                    match self.node(first).and_then(|n| n.parent.map(|p| (p, n.name.clone()))) {
                        Some((p, n)) => self.gather(&mut out, &mut seen, instances(self.h, p, &n))?,
                        None => self.gather(&mut out, &mut seen, [first])?,
                    }
                }
            }
            Expr::Member(base, name) => {
                let b = self.eval(base)?;
                for i in self.nodes_list(&b) {
                    self.gather(&mut out, &mut seen, instances(self.h, i, name))?;
                }
                if out.is_empty() {
                    let v = self.member(b, name)?;
                    out = self.nodes_list(&v);
                }
            }
            Expr::Descend(base, name) => {
                let b = self.eval(base)?;
                for i in self.nodes_list(&b) {
                    self.gather(&mut out, &mut seen, descendants_named(self.h, i, name, 0))?;
                }
            }
            other => {
                let v = self.eval(other)?;
                out = self.nodes_list(&v);
            }
        }
        Ok(out)
    }
}
