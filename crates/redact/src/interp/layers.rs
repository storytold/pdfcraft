use super::*;

impl<'a> Scope<'a> {
    /// Is this optional-content reference (an OCG or OCMD) hidden?
    fn layer_hidden(&mut self, doc: &Document, o: &Object) -> bool {
        if !self.layers_enabled {
            return false;
        }
        let reference = o.as_ref();
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
        let d = doc.resolve(o);
        let Some(d) = d.as_dict() else { return false };
        if d.name(b"Type") != Some(b"OCMD") {
            return false;
        }
        let groups: Vec<ObjRef> = match d.get(b"OCGs").map(|g| (*doc.resolve(g)).clone()) {
            Some(Object::Array(a)) => a.iter().filter_map(Object::as_ref).collect(),
            Some(_) => d.get(b"OCGs").and_then(Object::as_ref).into_iter().collect(),
            None => Vec::new(),
        };
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
            let policy = d.get(b"P").map(|p| doc.resolve(p)).and_then(|p| p.as_name().map(|name| name.to_vec()));
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
        let reference = expression.as_ref();
        if let Some(r) = reference
            && !path.insert(r)
        {
            return false;
        }
        let resolved = doc.resolve(expression);
        let complete = match &*resolved {
            Object::Dict(d) if d.name(b"Type") == Some(b"OCG") => {
                if let Some(r) = reference
                    && hidden_layers.contains(&r)
                {
                    found.insert(r);
                }
                reference.is_some()
            }
            Object::Array(items) => {
                let Some(Object::Name(operator)) = items.first() else { return false };
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
        let reference = expression.as_ref();
        if let Some(r) = reference
            && !path.insert(r)
        {
            return None;
        }
        let resolved = doc.resolve(expression);
        let result = match &*resolved {
            Object::Dict(d) if d.name(b"Type") == Some(b"OCG") => reference.map(|r| !self.hidden_layers.contains(&r)),
            Object::Array(items) if (2..=4097).contains(&items.len()) => {
                let operator = match items.first() {
                    Some(Object::Name(n)) => n.as_slice(),
                    _ => return None,
                };
                match operator {
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
                    b"Not" if items.len() == 2 => Some(!self.visibility_expression(doc, &items[1], depth + 1, path, budget)?),
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
        if let Some(r) = expression.as_ref() {
            if self.hidden_layers.contains(&r) {
                return true;
            }
            if !path.insert(r) {
                return true;
            }
        }
        let resolved = doc.resolve(expression);
        let found = match &*resolved {
            Object::Array(items) => items.iter().any(|item| self.expression_mentions_hidden(doc, item, depth + 1, path, budget)),
            Object::Dict(d) if d.name(b"Type") == Some(b"OCG") => expression.as_ref().is_none_or(|r| self.hidden_layers.contains(&r)),
            Object::Dict(_) => true,
            _ => false,
        };
        if let Some(r) = expression.as_ref() {
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
