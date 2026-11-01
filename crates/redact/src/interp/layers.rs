use super::{Dict, Document, HashSet, ObjRef, Object, Op, Scope};

fn resolve(doc: &Document, obj: &Object) -> Object {
    let mut cur = obj.clone();
    for _ in 0..16 {
        let Object::Ref(r) = cur else { break };
        cur = (*doc.get(r)).clone();
    }
    cur
}

fn target_ref(doc: &Document, obj: &Object) -> Option<ObjRef> {
    let mut cur = obj.as_ref()?;
    for _ in 0..16 {
        match &*doc.get(cur) {
            Object::Ref(next) => cur = *next,
            _ => return Some(cur),
        }
    }
    None
}

fn dict_name(doc: &Document, d: &Dict, key: &[u8]) -> Option<Vec<u8>> {
    d.get(key).and_then(|v| resolve(doc, v).as_name().map(<[u8]>::to_vec))
}

fn is_ocmd(doc: &Document, d: &Dict) -> bool {
    match dict_name(doc, d, b"Type").as_deref() {
        Some(b"OCMD") => true,
        Some(_) => false,
        None => d.contains(b"OCGs") || d.contains(b"P") || d.contains(b"VE"),
    }
}

fn is_ocg(doc: &Document, d: &Dict) -> bool {
    match dict_name(doc, d, b"Type").as_deref() {
        Some(b"OCG") => true,
        Some(_) => false,
        None => !is_ocmd(doc, d),
    }
}

impl<'a> Scope<'a> {
    /// Is this optional-content reference (an OCG or OCMD) hidden?
    fn layer_hidden(&mut self, doc: &Document, o: &Object) -> bool {
        if !self.layers_enabled {
            return false;
        }
        let reference = target_ref(doc, o);
        if let Some(r) = reference {
            if let Some(hidden) = self.ocmd_cache.get(&r) {
                return *hidden;
            }
            if self.hidden_layers.contains(&r) {
                self.ocmd_cache.insert(r, true);
                return true;
            }
        }
        // Evaluate the policy against the same default configuration used to select hidden
        // groups. A membership dictionary's default policy is AnyOn.
        let d = resolve(doc, o);
        let Some(d) = d.as_dict() else { return false };
        if !is_ocmd(doc, d) {
            return false;
        }
        let raw_groups: Vec<ObjRef> = match d.get(b"OCGs").map(|g| resolve(doc, g)) {
            Some(Object::Array(a)) => a.iter().filter_map(|item| target_ref(doc, item)).collect(),
            Some(_) => d.get(b"OCGs").and_then(|g| target_ref(doc, g)).into_iter().collect(),
            None => Vec::new(),
        };
        let groups: Vec<ObjRef> =
            raw_groups.into_iter().filter(|r| self.hidden_layers.contains(r) || doc.get(*r).as_dict().is_some_and(|g| is_ocg(doc, g))).collect();
        let fail_closed = || groups.iter().any(|g| self.hidden_layers.contains(g));
        let hidden = if let Some(expression) = d.get(b"VE") {
            let visible = self.visibility_expression(doc, expression, 0, &mut HashSet::new(), &mut 4096);
            match visible {
                Some(visible) => !visible,
                None => fail_closed() || self.expression_mentions_hidden(doc, expression, 0, &mut HashSet::new(), &mut 4096),
            }
        } else if groups.is_empty() {
            false
        } else {
            let any_on = groups.iter().any(|g| !self.hidden_layers.contains(g));
            let all_on = groups.iter().all(|g| !self.hidden_layers.contains(g));
            let policy = dict_name(doc, d, b"P");
            match (d.get(b"P"), policy.as_deref()) {
                (None, _) | (Some(_), Some(b"AnyOn")) => !any_on,
                (Some(_), Some(b"AllOn")) => !all_on,
                (Some(_), Some(b"AnyOff")) => all_on,
                (Some(_), Some(b"AllOff")) => any_on,
                // An invalid policy must not make a candidate hidden block survive sanitization.
                _ => fail_closed(),
            }
        };
        if !hidden {
            self.retained_off_layers.extend(groups.iter().filter(|g| self.hidden_layers.contains(g)).copied());
            if let Some(expression) = d.get(b"VE") {
                let mut referenced = HashSet::new();
                if Self::collect_hidden_expression_groups(doc, expression, &self.hidden_layers, 0, &mut HashSet::new(), &mut 4096, &mut referenced) {
                    self.retained_off_layers.extend(referenced);
                } else {
                    // If an expression cannot be fully walked, preserve every off group so that
                    // the content we kept does not change visibility after configuration cleanup.
                    self.retained_off_layers.extend(self.hidden_layers.iter().copied());
                }
            }
        }
        if let Some(r) = reference {
            self.ocmd_cache.insert(r, hidden);
        }
        hidden
    }

    pub(super) fn xobject_layer_hidden(&mut self, doc: &Document, xobject: ObjRef, oc: &Object) -> bool {
        if oc.as_ref().is_some() {
            return self.layer_hidden(doc, oc);
        }
        if let Some(hidden) = self.xobject_oc_cache.get(&xobject) {
            return *hidden;
        }
        let hidden = self.layer_hidden(doc, oc);
        self.xobject_oc_cache.insert(xobject, hidden);
        hidden
    }

    fn collect_hidden_expression_groups(
        doc: &Document,
        expression: &Object,
        hidden_layers: &HashSet<ObjRef>,
        depth: usize,
        path: &mut HashSet<ObjRef>,
        budget: &mut usize,
        found: &mut HashSet<ObjRef>,
    ) -> bool {
        if depth > 64 || *budget == 0 {
            return false;
        }
        *budget -= 1;
        let reference = if expression.as_ref().is_some() {
            let Some(r) = target_ref(doc, expression) else {
                return false;
            };
            if !path.insert(r) {
                return false;
            }
            Some(r)
        } else {
            None
        };
        let resolved = resolve(doc, expression);
        let complete = match &resolved {
            Object::Dict(d) if dict_name(doc, d, b"Type").as_deref() == Some(b"OCG") || reference.is_some_and(|r| hidden_layers.contains(&r)) => {
                if let Some(r) = reference
                    && hidden_layers.contains(&r)
                {
                    found.insert(r);
                }
                reference.is_some()
            }
            Object::Array(items) => {
                let Some(operator) = items.first().and_then(|op| resolve(doc, op).as_name().map(<[u8]>::to_vec)) else { return false };
                let operands = match operator.as_slice() {
                    b"And" | b"Or" if (2..=4097).contains(&items.len()) => items.get(1..),
                    b"Not" if items.len() == 2 => items.get(1..),
                    _ => None,
                };
                operands.is_some_and(|operands| {
                    operands.iter().all(|item| Self::collect_hidden_expression_groups(doc, item, hidden_layers, depth + 1, path, budget, found))
                })
            }
            _ => false,
        };
        if let Some(r) = reference {
            path.remove(&r);
        }
        complete
    }

    /// Evaluate an optional-content visibility expression (`/VE`) against the default config.
    /// Malformed, cyclic, or excessive expressions return `None` so the caller can fail closed.
    fn visibility_expression(
        &self,
        doc: &Document,
        expression: &Object,
        depth: usize,
        path: &mut HashSet<ObjRef>,
        budget: &mut usize,
    ) -> Option<bool> {
        if depth > 64 || *budget == 0 {
            return None;
        }
        *budget -= 1;
        let reference = if expression.as_ref().is_some() {
            let r = target_ref(doc, expression)?;
            if !path.insert(r) {
                return None;
            }
            Some(r)
        } else {
            None
        };
        let resolved = resolve(doc, expression);
        let result = match &resolved {
            Object::Dict(d)
                if dict_name(doc, d, b"Type").as_deref() == Some(b"OCG") || reference.is_some_and(|r| self.hidden_layers.contains(&r)) =>
            {
                reference.map(|r| !self.hidden_layers.contains(&r))
            }
            Object::Array(items) if (2..=4097).contains(&items.len()) => {
                let operator = items.first().and_then(|op| resolve(doc, op).as_name().map(<[u8]>::to_vec))?;
                match operator.as_slice() {
                    b"And" if items.len() >= 2 => {
                        let mut visible = true;
                        for item in items.iter().skip(1) {
                            visible &= self.visibility_expression(doc, item, depth + 1, path, budget)?;
                        }
                        Some(visible)
                    }
                    b"Or" if items.len() >= 2 => {
                        let mut visible = false;
                        for item in items.iter().skip(1) {
                            visible |= self.visibility_expression(doc, item, depth + 1, path, budget)?;
                        }
                        Some(visible)
                    }
                    b"Not" if items.len() == 2 => Some(!self.visibility_expression(doc, items.get(1)?, depth + 1, path, budget)?),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(r) = reference {
            path.remove(&r);
        }
        result
    }

    /// Conservatively find hidden groups in an expression that could not be evaluated. Unknown
    /// syntax, cycles, and exhausted budgets count as a possible hidden reference.
    fn expression_mentions_hidden(&self, doc: &Document, expression: &Object, depth: usize, path: &mut HashSet<ObjRef>, budget: &mut usize) -> bool {
        if depth > 64 || *budget == 0 {
            return true;
        }
        *budget -= 1;
        let reference = if expression.as_ref().is_some() {
            let Some(r) = target_ref(doc, expression) else {
                return true;
            };
            if self.hidden_layers.contains(&r) {
                return true;
            }
            if !path.insert(r) {
                return true;
            }
            Some(r)
        } else {
            None
        };
        let resolved = resolve(doc, expression);
        let found = match &resolved {
            Object::Array(items) => items.iter().any(|item| self.expression_mentions_hidden(doc, item, depth + 1, path, budget)),
            Object::Dict(d) if dict_name(doc, d, b"Type").as_deref() == Some(b"OCG") => reference.is_none_or(|r| self.hidden_layers.contains(&r)),
            Object::Dict(_) => true,
            _ => false,
        };
        if let Some(r) = reference {
            path.remove(&r);
        }
        found
    }
}

/// Remove marked-content blocks of hidden layers (`/OC /name BDC … EMC`, nested blocks
/// included). Returns the remaining operators and how many blocks went.
pub(super) fn strip_hidden_layers(doc: &Document, scope: &mut Scope<'_>, ops: Vec<Op>, props: &Dict) -> (Vec<Op>, usize) {
    let mut out = Vec::with_capacity(ops.len());
    // For each open marked-content block: does it hide its contents?
    let mut stack: Vec<bool> = Vec::new();
    let mut removed = 0;
    for op in ops {
        let hidden_now = stack.last().copied().unwrap_or(false);
        match op.op.as_slice() {
            b"BDC" | b"BMC" => {
                let this = op.is("BDC")
                    && op.name(0) == Some(b"OC")
                    && match op.operands.get(1) {
                        Some(Object::Name(n)) => props.get(n).is_some_and(|o| scope.layer_hidden(doc, o)),
                        Some(o) => scope.layer_hidden(doc, o),
                        None => false,
                    };
                if this && !hidden_now {
                    removed += 1;
                }
                stack.push(hidden_now || this);
                if !(hidden_now || this) {
                    out.push(op);
                }
            }
            b"EMC" => {
                let was = stack.pop().unwrap_or(false);
                if !was {
                    out.push(op);
                }
            }
            _ => {
                if !hidden_now {
                    out.push(op);
                }
            }
        }
    }
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Report;
    use crate::interp::Mode;
    use pdfcraft_cos::DecodeBudget;

    fn test_doc() -> Document {
        Document::new_empty()
    }

    #[test]
    fn ocmd_without_explicit_type_or_with_indirect_type_hides_off_layer() {
        let mut doc = test_doc();
        let mut ocg = Dict::new();
        ocg.set(b"Type".to_vec(), Object::Name(b"OCG".to_vec()));
        let ocg_ref = doc.add(Object::Dict(ocg));

        // OCMD 1 omits optional /Type (valid per ISO 32000-2 Table 97).
        let mut ocmd_no_type = Dict::new();
        ocmd_no_type.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ocg_ref)]));
        let ocmd1_ref = doc.add(Object::Dict(ocmd_no_type));

        // OCMD 2 uses an indirect /Type /OCMD and a 2-hop reference to the OCG.
        let type_ref = doc.add(Object::Name(b"OCMD".to_vec()));
        let ocg_hop = doc.add(Object::Ref(ocg_ref));
        let mut ocmd_ind_type = Dict::new();
        ocmd_ind_type.set(b"Type".to_vec(), Object::Ref(type_ref));
        ocmd_ind_type.set(b"OCGs".to_vec(), Object::Ref(ocg_hop));
        let ocmd2_ref = doc.add(Object::Dict(ocmd_ind_type));

        let mut props = Dict::new();
        props.set(b"M1".to_vec(), Object::Ref(ocmd1_ref));
        props.set(b"M2".to_vec(), Object::Ref(ocmd2_ref));

        let mut report = Report::default();
        let mut scope = Scope::new(&[], Mode::Apply, &mut report, DecodeBudget::page());
        scope.layers_enabled = true;
        scope.hidden_layers.insert(ocg_ref);

        let ops = pdfcraft_content::parse(b"/OC /M1 BDC (secret1) Tj EMC /OC /M2 BDC (secret2) Tj EMC (kept) Tj").ops;
        let (remaining, removed) = strip_hidden_layers(&doc, &mut scope, ops, &props);
        assert_eq!(removed, 2);
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn ocmd_ignores_deleted_or_non_dict_ocg_entries_under_any_on() {
        let mut doc = test_doc();
        let mut ocg = Dict::new();
        ocg.set(b"Type".to_vec(), Object::Name(b"OCG".to_vec()));
        let ocg_ref = doc.add(Object::Dict(ocg));
        let null_ref = doc.add(Object::Null);
        let int_ref = doc.add(Object::Int(42));

        let mut ocmd = Dict::new();
        ocmd.set(b"Type".to_vec(), Object::Name(b"OCMD".to_vec()));
        ocmd.set(b"P".to_vec(), Object::Name(b"AnyOn".to_vec()));
        ocmd.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ocg_ref), Object::Ref(null_ref), Object::Ref(int_ref)]));
        let ocmd_ref = doc.add(Object::Dict(ocmd));

        let mut report = Report::default();
        let mut scope = Scope::new(&[], Mode::Apply, &mut report, DecodeBudget::page());
        scope.layers_enabled = true;
        scope.hidden_layers.insert(ocg_ref);

        assert!(
            scope.layer_hidden(&doc, &Object::Ref(ocmd_ref)),
            "deleted/null and non-dictionary entries in /OCGs must be ignored per ISO 32000-2 Table 97"
        );
    }

    #[test]
    fn visibility_expression_resolves_indirect_operators_and_chained_ocg_refs() {
        let mut doc = test_doc();
        let ocg_type_ref = doc.add(Object::Name(b"OCG".to_vec()));
        let mut ocg_on = Dict::new();
        ocg_on.set(b"Type".to_vec(), Object::Ref(ocg_type_ref));
        let ocg_on_ref = doc.add(Object::Dict(ocg_on));
        let ocg_on_hop = doc.add(Object::Ref(ocg_on_ref));

        let not_op_ref = doc.add(Object::Name(b"Not".to_vec()));
        let ve_ref = doc.add(Object::Array(vec![Object::Ref(not_op_ref), Object::Ref(ocg_on_hop)]));

        let mut ocmd = Dict::new();
        ocmd.set(b"VE".to_vec(), Object::Ref(ve_ref));
        let ocmd_ref = doc.add(Object::Dict(ocmd));

        let mut report = Report::default();
        let mut scope = Scope::new(&[], Mode::Apply, &mut report, DecodeBudget::page());
        scope.layers_enabled = true;

        assert!(
            scope.layer_hidden(&doc, &Object::Ref(ocmd_ref)),
            "/VE [/Not <on-ocg>] with indirect operator and chained OCG ref must evaluate to hidden"
        );
    }

    #[test]
    fn cyclic_ocmd_and_ve_ref_chains_fail_closed_and_terminate() {
        let mut doc = test_doc();
        let mut ocg = Dict::new();
        ocg.set(b"Type".to_vec(), Object::Name(b"OCG".to_vec()));
        let ocg_ref = doc.add(Object::Dict(ocg));

        let cycle_a = doc.add(Object::Null);
        let cycle_b = doc.add(Object::Ref(cycle_a));
        doc.set(cycle_a, Object::Ref(cycle_b));

        let mut ocmd = Dict::new();
        ocmd.set(b"OCGs".to_vec(), Object::Array(vec![Object::Ref(ocg_ref), Object::Ref(cycle_a)]));
        ocmd.set(b"VE".to_vec(), Object::Ref(cycle_a));
        let ocmd_ref = doc.add(Object::Dict(ocmd));

        let mut report = Report::default();
        let mut scope = Scope::new(&[], Mode::Apply, &mut report, DecodeBudget::page());
        scope.layers_enabled = true;
        scope.hidden_layers.insert(ocg_ref);

        assert!(scope.layer_hidden(&doc, &Object::Ref(ocmd_ref)));
    }
}
