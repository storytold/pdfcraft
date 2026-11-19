use super::{
    CosError, DecodeBudget, Dict, Document, HashMap, HashSet, Kinds, MAX_DEPTH, Matrix, Metrics, ObjRef, Object, Op, Pieces, Rc, RedactError, Scope,
    Stream, is_vertical, kind, num, overlaps, page_streams, res_dict,
};

/// Drop every `replaced` name of the XObject resources `xo` that the rewritten `content` no
/// longer draws, so a full-rewrite save stops reaching the original image or form it named. A
/// name still drawn is kept: by a surviving `Do` (the same image shown again outside a mark), by
/// a form without /Resources of its own, which draws through `xo` too, or by a form with resources
/// of its own that draws it through its caller's names ([`resource_ops`]). Forms without /Resources
/// that `content`, or other such forms, draw are read within `budget`, what is left of the page's
/// allowance. Fails, as the cleanup of the fonts and properties these forms draw with does, when
/// that can't be told: such a form doesn't decode (an image codec or a /Filter it can't run, say)
/// or runs past `budget` (for the fonts' cleanup, an allowance of its own when no Type 3 font is
/// listed), the walk passes 2,000,000 operators, or the parser can't read all of such a form or of
/// `content`; or, with a name to drop, a form with resources of its own wasn't all read, or the
/// data of a form, with resources of its own or without, is damaged partway or, for Flate, cut
/// short, so that what it draws after the damage isn't known.
pub(crate) fn prune(
    doc: &Document,
    budget: &mut DecodeBudget,
    scoped: &mut ScopedReads,
    xo: &mut Dict,
    replaced: &[Vec<u8>],
    content: &[Vec<u8>],
) -> Result<(), CosError> {
    if replaced.is_empty() {
        return Ok(());
    }
    let res = Dict::from_iter([(b"XObject".to_vec(), Object::Dict(xo.clone()))]);
    let read = resource_ops_with(doc, budget, scoped, &res, None, content)?;
    let drawn: HashSet<&[u8]> = read.ops.iter().filter(|op| op.is("Do")).filter_map(Op::last_name).collect();
    for n in replaced {
        if !drawn.contains(n.as_slice()) {
            may_remove(read.partial)?;
            xo.remove(n);
        }
    }
    Ok(())
}

/// How many names one operation looks up in the scopes of forms whose own `/XObject` gives a
/// stream, or reads in those `/XObject`s ([`names_content_draws`]): each such form's are looked up
/// again in each of its scopes, and its `/XObject` read again for each dictionary checked.
const MAX_SCOPE_LOOKUPS: usize = 1 << 20;

/// The XObject names of the resource dictionary `xo` that any of `contents` draws: directly, or
/// through a form it draws. A form without /Resources of its own draws through its caller's names;
/// one with resources of its own draws a name its own `/XObject` gives as a stream from there, and
/// any other through its caller's, as the renderer looks names up ([`super::form_resources`]). Each
/// form is decoded once, with its `/DecodeParms` given by reference, and each form's own `/XObject`
/// read once, shared by the forms that give the same dictionary. The names a form draws are looked
/// up in each scope it is drawn in (the forms on the way to it whose own `/XObject` gives a stream),
/// however many names are asked about, and each distinct name is kept once. Look-ups in such a
/// form's scope, and the entries of each own `/XObject` read, are charged to `lookups`, what is
/// left of the operation's [`MAX_SCOPE_LOOKUPS`]; the rest are bounded by what the forms and
/// `contents` draw, as before. The flag says whether some of what is drawn could not be read (a
/// form that will not decode, bytes the parser can't read in one or in `contents`, nesting past the
/// limit, more than 65,536 scopes, or no look-ups left), so a name missing from the set may still
/// be drawn. Operators are read one at a time, since only the `Do`s matter: no stream's are held
/// whole.
fn names_content_draws(doc: &Document, contents: &[Vec<Vec<u8>>], xo: &Dict, lookups: &mut usize, kinds: &mut Kinds) -> (HashSet<Vec<u8>>, bool) {
    // The scopes names are looked up in: each the XObject names a form's own resources give as
    // streams, under its caller's scope. Scope 0 is that of `contents`, which look them up in `xo`
    // alone.
    // A form whose own resources give none looks every name up as its caller does, in its scope.
    const MAX_SCOPES: usize = 1 << 16;
    let mut names = Names { xo, ids: HashMap::new(), in_xo: Vec::new() };
    // The names each form draws, decoded once: `None` where it doesn't decode.
    let mut names_of: HashMap<ObjRef, Option<Rc<Vec<usize>>>> = HashMap::new();
    // Each form's own XObject names given as streams, `None` without resources of its own.
    let mut own_of: HashMap<ObjRef, Option<(OwnGiven, Rc<OwnNames>)>> = HashMap::new();
    let mut own_read: HashMap<OwnGiven, Rc<OwnNames>> = HashMap::new();
    let mut scopes: Vec<(Option<usize>, Rc<OwnNames>)> = vec![(None, Rc::default())];
    let mut children: HashMap<(usize, OwnGiven), usize> = HashMap::new();
    let mut drawn = HashSet::new();
    let mut handled = HashSet::new();
    let mut seen = HashSet::new();
    // A form that will not decode makes the answer uncertain, but it does not stop the forms
    // already queued from being read: a name drawn only inside one of those need not be unknown.
    let mut unknown = false;
    'read: for content in contents {
        let joined = Pieces::join(content);
        // What is still to read, each with its scope and depth: `content` itself (`None`), then the
        // forms it draws.
        let mut pending: Vec<(Option<ObjRef>, usize, usize)> = vec![(None, 0, 0)];
        while let Some((form, scope, depth)) = pending.pop() {
            let drawn_here = match form {
                None => Rc::new(names.drawn_by(joined.data(), &mut unknown)),
                Some(r) => {
                    let drawn_here = names_of.entry(r).or_insert_with(|| {
                        let obj = doc.get(r);
                        let Object::Stream(s) = &*obj else { return Some(Rc::new(Vec::new())) };
                        #[cfg(test)]
                        SHARED_FORM_READS.with(|c| c.set(c.get() + 1));
                        doc.with_direct_parms(s).decoded().ok().map(|data| Rc::new(names.drawn_by(&data, &mut unknown)))
                    });
                    match drawn_here {
                        Some(drawn_here) => drawn_here.clone(),
                        None => {
                            unknown = true;
                            continue;
                        }
                    }
                }
            };
            for &n in drawn_here.iter() {
                if scope != 0 {
                    let Some(left) = lookups.checked_sub(1) else {
                        unknown = true;
                        break 'read;
                    };
                    *lookups = left;
                }
                if !handled.insert((scope, n)) {
                    continue;
                }
                // The innermost form on the way that gives the name, or else `xo`.
                let mut at = Some(scope);
                let mut given = None;
                while let Some((parent, own)) = at.and_then(|i| scopes.get(i)) {
                    if let Some(&value) = own.get(&n) {
                        given = Some(value);
                        break;
                    }
                    at = *parent;
                }
                let value = match given {
                    Some(value) => value,
                    None => {
                        drawn.insert(n);
                        names.in_xo.get(n).copied().flatten()
                    }
                };
                let Some(r) = value else { continue };
                if !kind(doc, &Object::Ref(r), kinds).form {
                    continue;
                }
                let own = match own_of.get(&r) {
                    Some(own) => own.clone(),
                    None => {
                        let obj = doc.get(r);
                        let Object::Stream(s) = &*obj else { continue };
                        let Some(own) = own_streams(doc, s, r, &mut names, &mut own_read, lookups, kinds) else {
                            unknown = true;
                            break 'read;
                        };
                        own_of.insert(r, own.clone());
                        own
                    }
                };
                let child = match own {
                    Some((given, own)) if !own.is_empty() => match children.get(&(scope, given)) {
                        Some(&child) => child,
                        None if scopes.len() >= MAX_SCOPES => {
                            unknown = true;
                            continue;
                        }
                        None => {
                            #[cfg(test)]
                            SHARED_SCOPES.with(|c| c.set(c.get() + 1));
                            scopes.push((Some(scope), own));
                            children.insert((scope, given), scopes.len() - 1);
                            scopes.len() - 1
                        }
                    },
                    _ => scope,
                };
                if !seen.insert((child, r)) {
                    continue;
                }
                if depth < MAX_DEPTH {
                    pending.push((Some(r), child, depth + 1));
                } else {
                    unknown = true;
                }
            }
        }
    }
    (names.ids.into_iter().filter(|(_, n)| drawn.contains(n)).map(|(name, _)| name).collect(), unknown)
}

/// The names [`names_content_draws`] meets, each kept once and known by its number, with the
/// reference `xo` gives it, if any.
struct Names<'a> {
    xo: &'a Dict,
    ids: HashMap<Vec<u8>, usize>,
    in_xo: Vec<Option<ObjRef>>,
}

impl Names<'_> {
    fn id(&mut self, name: &[u8]) -> usize {
        if let Some(&n) = self.ids.get(name) {
            return n;
        }
        let n = self.in_xo.len();
        self.in_xo.push(self.xo.get(name).and_then(Object::as_ref));
        self.ids.insert(name.to_vec(), n);
        n
    }

    /// The names `data`'s `Do` operators draw, each once, setting `unknown` where the parser can't
    /// read all of `data`.
    fn drawn_by(&mut self, data: &[u8], unknown: &mut bool) -> Vec<usize> {
        let mut seen = HashSet::new();
        let mut read = pdfcraft_content::ops(data);
        let drawn = read.by_ref().filter(|op| op.is("Do")).filter_map(|op| op.last_name().map(|n| self.id(n))).filter(|n| seen.insert(*n)).collect();
        // Bytes the parser can't read may hold a `Do`.
        *unknown |= read.skipped() > 0;
        drawn
    }
}

/// A form's own XObject names that are streams, by name, with the reference each is given by (none
/// for a stream given directly).
type OwnNames = HashMap<usize, Option<ObjRef>>;

/// The dictionary a form's own XObject names are read from, by the first reference on the way to
/// it: its `/XObject` entry, else its `/Resources`, else the form itself.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum OwnGiven {
    XObject(ObjRef),
    Resources(ObjRef),
    Form(ObjRef),
}

/// The XObject names `form` (object `r`)'s own resources give as streams, read once for each
/// dictionary they come from (`read`), each entry read charged to `lookups`; inside, `None` when it
/// has no resources of its own: a /Resources that isn't a dictionary (null, or a missing object)
/// is none, as for `resource_ops` and the interpreter. `None` when `lookups` can't pay for them.
fn own_streams(
    doc: &Document,
    form: &Stream,
    r: ObjRef,
    names: &mut Names,
    read: &mut HashMap<OwnGiven, Rc<OwnNames>>,
    lookups: &mut usize,
    kinds: &mut Kinds,
) -> Option<Option<(OwnGiven, Rc<OwnNames>)>> {
    let Some(entry) = form.dict.get(b"Resources") else { return Some(None) };
    let resources = doc.resolve(entry);
    let Some(resources) = resources.as_dict() else { return Some(None) };
    let given = match (resources.get(b"XObject"), entry) {
        (Some(Object::Ref(x)), _) => OwnGiven::XObject(*x),
        (_, Object::Ref(res)) => OwnGiven::Resources(*res),
        _ => OwnGiven::Form(r),
    };
    if let Some(own) = read.get(&given) {
        return Some(Some((given, own.clone())));
    }
    *lookups = lookups.checked_sub(resources.get(b"XObject").map_or(0, |x| super::dict_len(doc, x)))?;
    let own: OwnNames =
        res_dict(doc, resources, b"XObject").iter().filter(|(_, v)| kind(doc, v, kinds).stream).map(|(k, v)| (names.id(k), v.as_ref())).collect();
    let own = Rc::new(own);
    read.insert(given, own.clone());
    Some(Some((given, own)))
}

#[cfg(test)]
thread_local! {
    /// How many forms [`names_content_draws`] has decoded.
    pub(crate) static SHARED_FORM_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many operators of forms with resources of their own [`resource_ops`] has read.
    pub(crate) static SCOPED_OPS_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many entries of forms' own resources [`resource_ops`] has copied.
    pub(crate) static OWN_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many scopes [`names_content_draws`] has opened, beyond each call's first.
    pub(crate) static SHARED_SCOPES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many scopes of forms with resources of their own [`resource_ops`] has opened.
    pub(crate) static CLEANUP_SCOPES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether any of `drawn` (from [`names_content_draws`]) draws `name`: `None` when none is known to and
/// one could not be read in full.
fn any_draws(drawn: &[(HashSet<Vec<u8>>, bool)], name: &[u8]) -> Option<bool> {
    if drawn.iter().any(|(names, _)| names.contains(name)) {
        Some(true)
    } else if drawn.iter().any(|(_, unknown)| *unknown) {
        None
    } else {
        Some(false)
    }
}

/// Every page-tree node with a /Resources of its own, and the pages that draw through it (the
/// nearest such node above a page, or the page itself). Cycles are cut as in
/// `pdfcraft_model::pages`. The flag is false when a node was reached twice (listed under two
/// parents, or in a cycle) or the walk stopped early: a page may then draw through a holder it
/// isn't listed under.
fn resource_holders(doc: &Document) -> (Vec<(ObjRef, Vec<ObjRef>)>, bool) {
    let mut holders: Vec<(ObjRef, Vec<ObjRef>)> = Vec::new();
    let mut whole = true;
    let Some(top) = doc.root().and_then(|root| doc.get(root).as_dict().and_then(|d| d.reference(b"Pages"))) else { return (holders, whole) };
    let mut seen = HashSet::new();
    let mut stack: Vec<(ObjRef, Option<usize>)> = vec![(top, None)];
    while let Some((node, mut holder)) = stack.pop() {
        if !seen.insert(node) || seen.len() > 1_000_000 {
            whole = false;
            continue;
        }
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        if d.contains(b"Resources") {
            holders.push((node, Vec::new()));
            holder = Some(holders.len() - 1);
        }
        let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
        if is_pages {
            let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
            stack.extend(kids.iter().filter_map(Object::as_ref).map(|k| (k, holder)));
        } else if let Some(h) = holder.and_then(|i| holders.get_mut(i)) {
            h.1.push(node);
        }
    }
    (holders, whole)
}

/// Unlink the image and form XObjects nothing draws from the page-tree resource dictionaries
/// that still name them: Delete Image and Replace Image take an image off the page but leave it
/// listed there, and so in a full-rewrite save. A name stays when anything that draws through the
/// dictionary uses it ([`names_drawn`]), and a dictionary whose use can't be told is left as it
/// is, as is every one when a page is listed twice in the page tree. A dictionary that loses an
/// entry is replaced by a private copy on its node, as in [`unlink_doomed`]. Returns how many
/// names went.
pub(crate) fn unlink_undrawn(doc: &mut Document) -> Result<usize, RedactError> {
    let index: HashMap<ObjRef, usize> = pdfcraft_model::pages(doc).iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let (holders, whole) = resource_holders(doc);
    if !whole {
        return Ok(0);
    }
    let mut n = 0;
    for (node, users) in holders {
        let Some(mut res) = doc.get(node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()) else {
            continue;
        };
        let mut xo = res_dict(doc, &res, b"XObject");
        let listed: Vec<Vec<u8>> = xo
            .iter()
            .filter(|(_, v)| {
                v.as_ref().is_some_and(|r| matches!(&*doc.get(r), Object::Stream(s) if matches!(s.dict.name(b"Subtype"), Some(b"Image" | b"Form"))))
            })
            .map(|(name, _)| name.to_vec())
            .collect();
        if listed.is_empty() {
            continue;
        }
        let mut pruned = false;
        // A form that goes may have been all that drew another name: look again while names go.
        for _ in 0..MAX_DEPTH {
            let Some(drawn) = names_drawn(doc, &res, &users, &index) else { break };
            let mut gone = 0;
            for name in listed.iter().filter(|name| !drawn.contains(*name)) {
                gone += usize::from(xo.remove(name).is_some());
            }
            if gone == 0 {
                break;
            }
            n += gone;
            pruned = true;
            res.set(b"XObject".to_vec(), Object::Dict(xo.clone()));
        }
        if pruned {
            doc.update_dict(node, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        }
    }
    Ok(n)
}

/// Every name used with `Do` by what draws through `res`, the resource dictionary of `pages`:
/// their content, their annotations' appearances, and every form, tiling pattern, Type 3 glyph
/// and soft mask those can reach, with /Resources of their own or not. A name such a stream
/// doesn't define is looked up in the resources of whatever draws it, up to the page's, so a `Do`
/// anywhere in there may draw from `res`. Going by name keeps more than it must, never less.
/// Each stream is read once, against one budget. `None` when that can't be told: a page or
/// stream that won't decode, or the budget spent.
fn names_drawn(doc: &Document, res: &Dict, pages: &[ObjRef], index: &HashMap<ObjRef, usize>) -> Option<HashSet<Vec<u8>>> {
    let mut drawn = HashSet::new();
    let mut note = |data: &[u8]| {
        for op in pdfcraft_content::parse(data).ops.iter().filter(|op| op.is("Do")) {
            if let Some(name) = op.last_name() {
                drawn.insert(name.to_vec());
            }
        }
    };
    // Streams still to read, and resource dictionaries still to look through.
    let mut streams: Vec<ObjRef> = Vec::new();
    let mut dicts: Vec<Dict> = vec![res.clone()];
    for page in pages {
        let dict = doc.get(*page).as_dict().cloned().unwrap_or_default();
        let (_, content) = page_streams(doc, &dict, index.get(page).copied().unwrap_or(0)).ok()?;
        // One stream may end with a name its `Do` begins the next with: read them as one.
        note(Pieces::join(&content).data());
        let annots = dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for annot in annots {
            let ap = doc.resolve(&annot).as_dict().and_then(|a| a.get(b"AP")).map(|ap| doc.resolve(ap)).and_then(|ap| ap.as_dict().cloned());
            // Each of /N, /R and /D is an appearance, or a dictionary of them by state.
            for (_, look) in ap.unwrap_or_default().iter() {
                match look.as_ref() {
                    Some(r) if matches!(&*doc.resolve(look), Object::Stream(_)) => streams.push(r),
                    _ => streams.extend(doc.resolve(look).as_dict().into_iter().flat_map(|states| states.iter().filter_map(|(_, v)| v.as_ref()))),
                }
            }
        }
    }
    let mut budget = DecodeBudget::page();
    let mut read = HashSet::new();
    let mut looked = 0;
    loop {
        while let Some(dict) = dicts.get(looked) {
            let (found, fonts) = drawn_through(doc, dict);
            looked += 1;
            streams.extend(found);
            // Type 3 fonts' own resources, each look charged like a form so fonts naming each
            // other can't go round for ever.
            for font_res in fonts {
                if !budget.enter_form() {
                    return None;
                }
                dicts.push(font_res);
            }
        }
        let Some(r) = streams.pop() else { break };
        if !read.insert(r) {
            continue;
        }
        let obj = doc.resolve(&Object::Ref(r));
        let Object::Stream(s) = &*obj else { continue };
        if !budget.enter_form() {
            return None;
        }
        note(&budget.decode_strict(doc, s).ok()?);
        if let Some(own) = s.dict.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()) {
            dicts.push(own);
        }
    }
    Some(drawn)
}

/// The content streams the resource dictionary `res` lists: its form XObjects, its tiling
/// patterns, its Type 3 fonts' glyph procedures and its soft masks' groups. Then the /Resources
/// of those Type 3 fonts, which their glyphs look names up in first. An object that holds only a
/// reference stands for what it names, as the renderer reads it.
fn drawn_through(doc: &Document, res: &Dict) -> (Vec<ObjRef>, Vec<Dict>) {
    let is_stream = |r: &ObjRef| matches!(&*doc.resolve(&Object::Ref(*r)), Object::Stream(_));
    let mut found: Vec<ObjRef> = Vec::new();
    let mut fonts: Vec<Dict> = Vec::new();
    for (_, x) in res_dict(doc, res, b"XObject").iter() {
        found.extend(x.as_ref().filter(|_| matches!(&*doc.resolve(x), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form"))));
    }
    for (_, p) in res_dict(doc, res, b"Pattern").iter() {
        found.extend(p.as_ref().filter(|_| matches!(&*doc.resolve(p), Object::Stream(s) if s.dict.int(b"PatternType") == Some(1))));
    }
    for (_, f) in res_dict(doc, res, b"Font").iter() {
        let font = doc.resolve(f);
        let Some(font) = font.as_dict().filter(|f| f.name(b"Subtype") == Some(b"Type3")) else { continue };
        let procs = font.get(b"CharProcs").map(|c| doc.resolve(c)).and_then(|c| c.as_dict().cloned()).unwrap_or_default();
        found.extend(procs.iter().filter_map(|(_, v)| v.as_ref()).filter(is_stream));
        fonts.extend(font.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()));
    }
    for (_, g) in res_dict(doc, res, b"ExtGState").iter() {
        let gs = doc.resolve(g);
        let mask = gs.as_dict().and_then(|g| g.get(b"SMask")).map(|m| doc.resolve(m));
        found.extend(mask.as_ref().and_then(|m| m.as_dict()).and_then(|m| m.reference(b"G")).filter(is_stream));
    }
    (found, fonts)
}

/// Unlink the XObjects a rewrite dropped or replaced (`doomed`) from every page-tree resource
/// dictionary that still names them. The rewritten page has its own pruned /Resources, but the
/// originals stay reachable, and so stay in a full-rewrite save, when:
/// - the page inherited its /Resources from a /Pages node, which still lists them;
/// - several pages share one resource dictionary, and another page still refers to it;
/// - a form drew them through the page's dictionary: one without /Resources, or one whose own lack
///   the name.
///
/// An entry goes unless a page that draws through that dictionary still draws the name (the same
/// image shown on another page, or again outside a mark). A dictionary that loses an entry is
/// replaced by a private copy on its node, so a dictionary object shared with other nodes is
/// left as it is for them. Retired properties are likewise unlinked only when none of the
/// holder's pages or inheriting forms names them. Fails closed when use cannot be determined.
pub(crate) fn unlink_doomed(doc: &mut Document, doomed: &[ObjRef], retired_properties: &[Object]) -> Result<(), RedactError> {
    crate::type3::unlink_type3(doc)?;
    if doomed.is_empty() && retired_properties.is_empty() {
        return Ok(());
    }
    let doomed: HashSet<ObjRef> = doomed.iter().copied().collect();
    let (holders, _) = resource_holders(doc);
    let index: HashMap<ObjRef, usize> = pdfcraft_model::pages(doc).iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let holder_nodes: Vec<ObjRef> = holders.iter().map(|(node, _)| *node).collect();
    let mut lookups = MAX_SCOPE_LOOKUPS;
    // What the objects forms give in their resources are, read once for the operation.
    let mut kinds = Kinds::new();
    for (node, users) in holders {
        let Some(mut res) = doc.get(node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()) else {
            continue;
        };
        let mut xo = res_dict(doc, &res, b"XObject");
        let names: Vec<Vec<u8>> = xo.iter().filter(|(_, v)| v.as_ref().is_some_and(|r| doomed.contains(&r))).map(|(n, _)| n.to_vec()).collect();
        let properties = res_dict(doc, &res, b"Properties");
        let property_names: Vec<Vec<u8>> = properties.iter().filter(|(_, v)| retired_properties.contains(v)).map(|(n, _)| n.to_vec()).collect();
        if names.is_empty() && property_names.is_empty() {
            continue;
        }
        let mut contents = Vec::with_capacity(users.len());
        for user in &users {
            let page = doc.get(*user).as_dict().cloned().unwrap_or_default();
            // A page whose content cannot be read may still draw the name: fail closed.
            contents.push(page_streams(doc, &page, index.get(user).copied().unwrap_or(0))?.1);
        }
        let mut pruned = false;
        // What the pages draw through this dictionary, worked out once for all of them and all the names.
        let drawn: Vec<_> = if names.is_empty() { Vec::new() } else { vec![names_content_draws(doc, &contents, &xo, &mut lookups, &mut kinds)] };
        for n in names {
            match any_draws(&drawn, &n) {
                Some(true) => {}
                Some(false) => {
                    xo.remove(&n);
                    pruned = true;
                }
                None => return Err(RedactError::Residue(1)),
            }
        }
        if pruned {
            res.set(b"XObject".to_vec(), Object::Dict(xo));
        }
        pruned |= prune_retired_properties(doc, &mut res, &property_names, &contents)?;
        if pruned {
            doc.update_dict(node, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
        }
    }
    // A surviving form can own the same old resource dictionary as a rewritten page. Its
    // unused descriptions and doomed XObjects remain reachable too, even after every page
    // holder is cleaned.
    unlink_from_forms(doc, &holder_nodes, &doomed, retired_properties, &mut lookups, &mut kinds)?;
    Ok(())
}

/// Retire only properties removed from a rewritten scope, preserving names still used by any
/// content sharing this holder. Different pages' streams must never be concatenated together.
fn prune_retired_properties(doc: &Document, res: &mut Dict, names: &[Vec<u8>], contents: &[Vec<Vec<u8>>]) -> Result<bool, CosError> {
    if names.is_empty() {
        return Ok(false);
    }
    let mut used = HashSet::new();
    let mut partial = false;
    for content in contents {
        // Each is one page's content or one form's, with the allowance of a page.
        let read = resource_ops(doc, &mut DecodeBudget::page(), res, content)?;
        partial |= read.partial;
        for op in read.ops {
            if matches!(op.op.as_slice(), b"BDC" | b"DP")
                && let Some(name) = op.last_name()
            {
                used.insert(name.to_vec());
            }
        }
    }
    let mut properties = res_dict(doc, res, b"Properties");
    let mut pruned = false;
    for name in names {
        if !used.contains(name) {
            may_remove(partial)?;
            pruned |= properties.remove(name).is_some();
        }
    }
    if pruned {
        if properties.is_empty() {
            res.remove(b"Properties");
        } else {
            res.set(b"Properties".to_vec(), Object::Dict(properties));
        }
    }
    Ok(pruned)
}

/// Unlink retired properties and `doomed` XObjects from the resources of the forms reachable from
/// the page-tree `holders`, each unless the form's own content (or a form inheriting its
/// resources) still uses the name. Fails closed when that cannot be determined. Look-ups in the
/// scopes of forms these forms draw are charged to `lookups` ([`names_content_draws`]).
fn unlink_from_forms(
    doc: &mut Document,
    holders: &[ObjRef],
    doomed: &HashSet<ObjRef>,
    retired: &[Object],
    lookups: &mut usize,
    kinds: &mut Kinds,
) -> Result<(), CosError> {
    if retired.is_empty() && doomed.is_empty() {
        return Ok(());
    }
    let mut pending: Vec<Dict> = holders
        .iter()
        .filter_map(|r| doc.get(*r).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()))
        .collect();
    let mut seen = HashSet::new();
    while let Some(resources) = pending.pop() {
        for (_, value) in res_dict(doc, &resources, b"XObject").iter() {
            let Some(reference) = value.as_ref() else { continue };
            if !seen.insert(reference) {
                continue;
            }
            if seen.len() > 1_000_000 {
                return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect shared form resources".into() });
            }
            let object = doc.get(reference);
            let Object::Stream(form) = &*object else { continue };
            if form.dict.name(b"Subtype") != Some(b"Form") {
                continue;
            }
            let Some(mut res) = form.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()) else { continue };
            let names: Vec<Vec<u8>> =
                res_dict(doc, &res, b"Properties").iter().filter(|(_, value)| retired.contains(value)).map(|(name, _)| name.to_vec()).collect();
            let mut xo = res_dict(doc, &res, b"XObject");
            let doomed_names: Vec<Vec<u8>> =
                xo.iter().filter(|(_, v)| v.as_ref().is_some_and(|r| doomed.contains(&r))).map(|(n, _)| n.to_vec()).collect();
            if names.is_empty() && doomed_names.is_empty() {
                pending.push(res);
                continue;
            }
            let content = [vec![doc.with_direct_parms(form).decoded()?]];
            let mut pruned = prune_doomed_xobjects(doc, &content, &mut xo, &doomed_names, lookups, kinds)?;
            if pruned {
                res.set(b"XObject".to_vec(), Object::Dict(xo));
            }
            pruned |= prune_retired_properties(doc, &mut res, &names, &content)?;
            if pruned {
                doc.update_dict(reference, |d| d.set(b"Resources".to_vec(), Object::Dict(res.clone())))?;
            }
            pending.push(res);
        }
    }
    Ok(())
}

fn prune_doomed_xobjects(
    doc: &Document,
    content: &[Vec<Vec<u8>>],
    xo: &mut Dict,
    doomed_names: &[Vec<u8>],
    lookups: &mut usize,
    kinds: &mut Kinds,
) -> Result<bool, CosError> {
    let mut pruned = false;
    if doomed_names.is_empty() {
        return Ok(false);
    }
    let drawn = [names_content_draws(doc, content, xo, lookups, kinds)];
    for name in doomed_names {
        match any_draws(&drawn, name) {
            Some(true) => {}
            Some(false) => pruned |= xo.remove(name).is_some(),
            None => return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect shared form resources".into() }),
        }
    }
    Ok(pruned)
}

/// The clip that hides everything inside each region: one even-odd clip per region (a big
/// rectangle with the region cut out), in the current user space. Returns `None` when the CTM
/// can't be inverted (nothing is painted then anyway).
pub(super) fn clip_out(rects: &[[f64; 4]], ctm: &Matrix, around: [f64; 4]) -> Option<Vec<Op>> {
    let inv = ctm.invert()?;
    let mut ops = Vec::new();
    let big = [around[0] - 10.0, around[1] - 10.0, around[2] + 10.0, around[3] + 10.0];
    for r in rects.iter().filter(|r| overlaps(**r, around, 0.0)) {
        let outer = [(big[0], big[1]), (big[2], big[1]), (big[2], big[3]), (big[0], big[3])];
        let inner = [(r[0], r[1]), (r[0], r[3]), (r[2], r[3]), (r[2], r[1])];
        for poly in [outer, inner] {
            for (k, (x, y)) in poly.iter().enumerate() {
                let (u, v) = inv.apply(*x, *y);
                ops.push(Op::new(if k == 0 { "m" } else { "l" }, vec![num(u), num(v)]));
            }
            ops.push(Op::new("h", vec![]));
        }
        ops.push(Op::new("W*", vec![]));
        ops.push(Op::new("n", vec![]));
    }
    Some(ops)
}

/// The operators of `content`, and of the forms it draws through `res`, nested ones too, that look
/// names up in `res`: what draws with these resources.
/// - **A form without /Resources of its own** draws through its caller's names: all its operators.
///   Each such form is read once, with its `/DecodeParms` given by reference, its decoded bytes
///   charged to `budget`, so the cleanups of one page that walk the same forms spend one allowance
///   between them (all but the fonts' cleanup when no Type 3 font is listed, which has one of its
///   own). An error when that can't all be read: the parser can't read all of `content` or of such
///   a form, the walk passes 2,000,000 operators, or such a form doesn't decode or runs past `budget`.
///   One damaged partway is read as far as the damage, and makes the read [`Drawn::partial`].
/// - **A form with resources of its own** looks a name up in its caller's where neither its own nor
///   those of the forms on the way to it give it, as the renderer does ([`super::form_resources`]):
///   of its operators, those naming an XObject (`Do`), font (`Tf`), graphics state (`gs`) or pattern
///   (`scn`, `SCN`) its own resources don't give as what that category holds. Its marked content's
///   properties are its own; those of a form without resources of its own inside it are the page's
///   (ISO 32000-2 §7.8.3), and are kept. Such forms are read once in each scope (the forms on the
///   way with resources of their own), not inside themselves again nor deeper than [`MAX_DEPTH`] of
///   them, where the renderer draws nothing either (it draws no form inside itself, nor anything
///   nested more than 50 deep), within an allowance of a page of their own, 2,000,000 of their
///   operators and 65,536 scopes. One that doesn't decode isn't read, nor past damage in its data,
///   bytes the parser can't read in one are passed over, and past a limit the later ones aren't
///   read: none of these fails the read, but each makes it [`Drawn::partial`], and a cleanup that
///   would then remove a name fails ([`may_remove`]), as such a form may draw with it.
///
/// Operators are read one at a time, and of forms with resources of their own only those kept are
/// held, so the walk stops at one past a limit rather than holding them all.
pub(crate) fn resource_ops(doc: &Document, budget: &mut DecodeBudget, res: &Dict, content: &[Vec<u8>]) -> Result<Drawn, CosError> {
    resource_ops_with(doc, budget, &mut ScopedReads::default(), res, None, content)
}

/// [`resource_ops`], its reads of forms with resources of their own spending `scoped`, which the
/// reads made for one page share. `drawn_through` is where `content` looks XObject names up when
/// that isn't `res`'s `/XObject` alone: a form with resources of its own draws a name they don't
/// give as a stream from its caller's ([`super::form_xobject_lookup`]).
pub(crate) fn resource_ops_with(
    doc: &Document,
    budget: &mut DecodeBudget,
    scoped: &mut ScopedReads,
    res: &Dict,
    drawn_through: Option<&Dict>,
    content: &[Vec<u8>],
) -> Result<Drawn, CosError> {
    let joined = Pieces::join(content);
    let mut read = pdfcraft_content::ops(joined.data());
    let ops: Vec<Op> = read.by_ref().take(MAX_OPS + 1).collect();
    // Under the limit the reader ran to the end, so `skipped` counts all of it.
    if read.skipped() > 0 || ops.len() > MAX_OPS {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully read the content that uses these resources".into() });
    }
    match drawn_through {
        Some(xobjects) => resource_ops_from(doc, budget, scoped, xobjects, ops),
        None => resource_ops_from(doc, budget, scoped, &res_dict(doc, res, b"XObject"), ops),
    }
}

/// What the reads of forms with resources of their own ([`resource_ops`]) spend between them where
/// they share it, as the reads made for one page do: an allowance of a page and 2,000,000 of their
/// operators, past which they are partial ([`Drawn::partial`]), and what is worked out of them.
pub(crate) struct ScopedReads {
    budget: DecodeBudget,
    ops: usize,
    cache: ReadCache,
    /// Each such form read.
    read: HashMap<ObjRef, Rc<FormRead>>,
}

impl Default for ScopedReads {
    fn default() -> Self {
        ScopedReads { budget: DecodeBudget::page(), ops: 0, cache: ReadCache::default(), read: HashMap::new() }
    }
}

impl ScopedReads {
    /// Nothing left: such forms aren't read, and a read that meets one is partial.
    pub(crate) fn spent() -> Self {
        ScopedReads { ops: MAX_OPS.saturating_add(1), ..ScopedReads::default() }
    }
}

/// What [`resource_ops_from`] works out of the forms it reads, kept for the reads that share it
/// ([`ScopedReads`]): each form's own resources, in the categories it looks names up in, each
/// category copied once for however many forms give it the same way ([`CategoryGiven`]), what the
/// objects they give are, and which fonts and graphics states set a Type 3 font without resources
/// of its own ([`sets_type3`]).
#[derive(Default)]
struct ReadCache {
    own_of: HashMap<ObjRef, Rc<OwnResources>>,
    copies: HashMap<CategoryGiven, Rc<Dict>>,
    kinds: Kinds,
    type3: HashMap<(ObjRef, Category), bool>,
}

/// How a form gives one of its own categories, so that forms giving it the same way share one copy:
/// by the reference its entry holds, else inside the `/Resources` it gives by reference, else
/// written in the form itself.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum CategoryGiven {
    Entry(ObjRef),
    Resources(ObjRef, Category),
    Form(ObjRef, Category),
}

/// How many operators [`resource_ops`] reads: of the content and the forms without resources of
/// their own that it draws, and, apart, of forms with resources of their own.
const MAX_OPS: usize = 2_000_000;

/// [`resource_ops_with`] from operators already read, `ops`, that draw through the XObject names
/// `xobjects`.
pub(crate) fn resource_ops_from(
    doc: &Document,
    budget: &mut DecodeBudget,
    scoped: &mut ScopedReads,
    xobjects: &Dict,
    mut ops: Vec<Op>,
) -> Result<Drawn, CosError> {
    const MAX_SCOPES: usize = 1 << 16;
    // The scopes of forms with resources of their own: each the form's own names over its caller's
    // scope (`None`: `res`'s alone).
    let mut scopes: Vec<Scoped> = Vec::new();
    let mut scope_ids: HashMap<(Option<usize>, ObjRef), usize> = HashMap::new();
    // The XObjects drawn, each with the scope it is looked up in, to follow in turn.
    let mut draws: Vec<(Vec<u8>, Option<usize>)> =
        ops.iter().filter(|op| op.is("Do")).filter_map(|op| op.last_name()).map(|n| (n.to_vec(), None)).collect();
    let mut seen = HashSet::new();
    let cache = &mut scoped.cache;
    let mut partial = false;
    let mut type3 = false;
    let mut cursor = 0;
    while let Some((name, scope)) = draws.get(cursor).cloned() {
        cursor += 1;
        let entry = given_in(doc, &scopes, scope, Category::XObject, &name, &mut cache.kinds).or_else(|| xobjects.get(&name).cloned());
        let Some(r) = entry.as_ref().and_then(Object::as_ref) else { continue };
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        if s.dict.name(b"Subtype") != Some(b"Form") {
            continue;
        }
        // Read in place, so a form drawn again and again isn't copied each time.
        let resources = s.dict.get(b"Resources");
        let held = resources.filter(|o| matches!(o, Object::Ref(_))).map(|o| doc.resolve(o));
        let own = match (&held, resources) {
            (Some(held), _) => held.as_dict(),
            (None, Some(direct)) => direct.as_dict(),
            (None, None) => None,
        };
        // A form without resources of its own reads the page's properties (ISO 32000-2 §7.8.3).
        let page_properties = own.is_none();
        let child = match own {
            None => scope,
            Some(_) if scoped.ops > MAX_OPS => {
                partial = true;
                continue;
            }
            Some(own) => match scope_ids.get(&(scope, r)) {
                Some(&child) => Some(child),
                // The renderer draws no form inside itself, and the forms on the way are followed no
                // deeper than it nests forms (each of these is one level of its nesting, or more).
                None if on_chain(&scopes, scope, r) => continue,
                None if scope.and_then(|i| scopes.get(i)).is_some_and(|s| s.depth >= MAX_DEPTH) => continue,
                None if scopes.len() >= MAX_SCOPES => {
                    partial = true;
                    continue;
                }
                None => {
                    let given = resources.and_then(Object::as_ref);
                    let own = match cache.own_of.get(&r) {
                        Some(own) => own.clone(),
                        None => {
                            let read = Rc::new(OwnResources::of(doc, own, given, r, &mut cache.copies));
                            cache.own_of.insert(r, read.clone());
                            read
                        }
                    };
                    let depth = scope.and_then(|i| scopes.get(i)).map_or(1, |s| s.depth + 1);
                    #[cfg(test)]
                    CLEANUP_SCOPES.with(|c| c.set(c.get() + 1));
                    scopes.push(Scoped { parent: scope, own, owner: r, depth });
                    scope_ids.insert((scope, r), scopes.len() - 1);
                    Some(scopes.len() - 1)
                }
            },
        };
        if !seen.insert((r, child)) {
            continue;
        }
        if child.is_none() {
            // A stream damaged partway decodes to what came before the damage: not all of it.
            let (data, damaged) = budget.decode_with_damage(doc, s, usize::MAX)?;
            partial |= damaged;
            let mut read = pdfcraft_content::ops(&data);
            for op in read.by_ref() {
                if op.is("Do")
                    && let Some(n) = op.last_name()
                {
                    draws.push((n.to_vec(), None));
                }
                ops.push(op);
                // `draws` gains at most one name for each operator kept, so this bounds it too.
                if ops.len() > MAX_OPS {
                    return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect inherited form resources".into() });
                }
            }
            if read.skipped() > 0 {
                return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully inspect inherited form resources".into() });
            }
            continue;
        }
        // Read once for the reads that share `scoped`: what the form draws, and of its operators
        // those its own resources don't give the name of (all, without resources of its own); each
        // read then keeps those the forms on the way don't give either.
        let read = match scoped.read.get(&r) {
            Some(read) => {
                // Taken again, it costs what it hands on.
                scoped.ops = scoped.ops.saturating_add(read.weight);
                if scoped.ops > MAX_OPS {
                    partial = true;
                    continue;
                }
                read.clone()
            }
            None => {
                let mut read = FormRead::default();
                match scoped.budget.decode_with_damage(doc, s, usize::MAX) {
                    Err(_) => read.partial = true,
                    Ok((data, damaged)) => {
                        read.partial |= damaged;
                        let mut parsed = pdfcraft_content::ops(&data);
                        for op in parsed.by_ref() {
                            scoped.ops += 1;
                            #[cfg(test)]
                            SCOPED_OPS_READ.with(|c| c.set(c.get() + 1));
                            if scoped.ops > MAX_OPS {
                                read.partial = true;
                                break;
                            }
                            if op.is("Do")
                                && let Some(n) = op.last_name()
                            {
                                read.draws.push(n.to_vec());
                            }
                            let marks = page_properties && matches!(op.op.as_slice(), b"BDC" | b"DP");
                            let own = child.and_then(|i| scopes.get(i)).filter(|_| !page_properties);
                            let looked = Category::of(&op)
                                .map(|(category, name)| (category, own.and_then(|own| own.own.gives(doc, category, name, &mut cache.kinds))));
                            match looked {
                                Some((category, Some(entry))) => read.type3 |= sets_type3(doc, category, &entry, &mut cache.type3),
                                Some((_, None)) => read.kept.push(slim(op)),
                                None if marks => read.kept.push(slim(op)),
                                None => {}
                            }
                        }
                        read.partial |= parsed.skipped() > 0;
                    }
                }
                read.draws.sort_unstable();
                read.draws.dedup();
                // The cleanups ask only whether an operator is there, so each is handed on once.
                let order =
                    |a: &Op, b: &Op| a.op.cmp(&b.op).then_with(|| a.operands.iter().map(Object::as_name).cmp(b.operands.iter().map(Object::as_name)));
                read.kept.sort_unstable_by(order);
                read.kept.dedup_by(|a, b| order(a, b).is_eq());
                let names = |sum: usize, name: &[u8]| sum.saturating_add(name.len()).saturating_add(1);
                let drawn = read.draws.iter().fold(1, |sum, name| names(sum, name));
                read.weight =
                    read.kept.iter().flat_map(|op| &op.operands).filter_map(Object::as_name).fold(drawn.saturating_add(read.kept.len()), names);
                let read = Rc::new(read);
                scoped.read.insert(r, read.clone());
                read
            }
        };
        partial |= read.partial;
        type3 |= read.type3;
        draws.extend(read.draws.iter().map(|n| (n.clone(), child)));
        for op in &read.kept {
            let marks = page_properties && matches!(op.op.as_slice(), b"BDC" | b"DP");
            match Category::of(op).map(|(category, name)| (category, given_in(doc, &scopes, child, category, name, &mut cache.kinds))) {
                Some((category, Some(entry))) => type3 |= sets_type3(doc, category, &entry, &mut cache.type3),
                Some((_, None)) => ops.push(op.clone()),
                None if marks => ops.push(op.clone()),
                None => {}
            }
        }
    }
    Ok(Drawn { ops, partial, type3 })
}

/// Whether what `drawn` reads sets a Type 3 font without resources of its own ([`sets_type3`]):
/// through the names of a form on the way ([`Drawn::type3`]), or by an operator of `drawn.ops`
/// naming one that `res`, the names read, give. Each name is looked up once, and each object read
/// once for the reads that share `scoped`.
pub(crate) fn sets_type3_drawn(doc: &Document, scoped: &mut ScopedReads, res: &Dict, drawn: &Drawn) -> bool {
    if drawn.type3 {
        return true;
    }
    let fonts = res.get(b"Font").map(|names| doc.resolve(names));
    let states = res.get(b"ExtGState").map(|names| doc.resolve(names));
    let mut asked = HashSet::new();
    drawn.ops.iter().filter_map(Category::of).any(|(category, name)| {
        let names = match category {
            Category::Font => &fonts,
            Category::ExtGState => &states,
            Category::XObject | Category::Pattern => return false,
        };
        asked.insert((category, name))
            && names
                .as_ref()
                .and_then(|names| names.as_dict())
                .and_then(|names| names.get(name))
                .is_some_and(|entry| sets_type3(doc, category, entry, &mut scoped.cache.type3))
    })
}

/// Whether `entry`, given for `category`, sets a Type 3 font without resources of its own, whose
/// glyphs then paint through the names of what shows them: such a font (`Tf`), or a graphics state
/// whose `/Font` is one (`gs`). Each object is read once, into `cache`.
fn sets_type3(doc: &Document, category: Category, entry: &Object, cache: &mut HashMap<(ObjRef, Category), bool>) -> bool {
    let read = || match category {
        Category::Font => doc.resolve(entry).as_dict().is_some_and(|font| super::type3_without_resources(doc, font)),
        Category::ExtGState => super::state_sets_type3(doc, entry),
        Category::XObject | Category::Pattern => false,
    };
    match entry {
        Object::Ref(r) => *cache.entry((*r, category)).or_insert_with(read),
        _ => read(),
    }
}

/// `op` as the cleanups read a kept operator: its keyword, and of its operands the last two, each
/// where it is a name and as null otherwise (the last is the name most operators look up, the one
/// before it a `Tf`'s font). The rest isn't read, so it isn't kept, nor handed on again.
fn slim(op: Op) -> Op {
    let from = op.operands.len().saturating_sub(2);
    let operands = op.operands.into_iter().skip(from).map(|o| if matches!(o, Object::Name(_)) { o } else { Object::Null }).collect();
    Op { operands, inline: None, ..op }
}

/// What reading a form with resources of its own, or one drawn inside such a form, gave: the names
/// it draws, each once, the operators that may look a name up past it ([`resource_ops`]) as
/// [`slim`] keeps them, each once, whether it sets a Type 3 font without resources of its own that
/// its own resources give ([`sets_type3`]), whether it wasn't all read, and what taking it again
/// costs of [`MAX_OPS`]: one for each operator, one for each name with its bytes, and one.
#[derive(Default)]
struct FormRead {
    draws: Vec<Vec<u8>>,
    kept: Vec<Op>,
    type3: bool,
    partial: bool,
    weight: usize,
}

/// What content draws with a resource dictionary ([`resource_ops`]).
pub(crate) struct Drawn {
    /// The operators that look names up in it.
    pub(crate) ops: Vec<Op>,
    /// Whether a form the content draws wasn't all read (one with resources of its own, or one
    /// whose data is damaged), so that it may still draw a name `ops` doesn't.
    pub(crate) partial: bool,
    /// Whether such a form, or one inside it, sets a Type 3 font without resources of its own that
    /// the names of a form on the way give ([`sets_type3`]); one that the names read give is set by
    /// an operator among `ops`.
    pub(crate) type3: bool,
}

/// Before a cleanup removes a name what it read doesn't use: an error when that read was
/// [`Drawn::partial`]. Removing the name could lose what an unread form draws with it, and keeping
/// a replaced one would keep what was redacted, so the operation is refused.
pub(crate) fn may_remove(partial: bool) -> Result<(), CosError> {
    if partial {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not fully read a form that may draw with what it would remove".into() });
    }
    Ok(())
}

/// The resource categories a form looks a name up in its caller's for, as [`resource_ops`] reads
/// operators: the walk's ([`super::COPIED`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Category {
    XObject,
    Font,
    ExtGState,
    Pattern,
}

impl Category {
    /// The category `op` looks its name up in, and the name, for an operator that names a resource
    /// a form may take from its caller's.
    fn of(op: &Op) -> Option<(Category, &[u8])> {
        match op.op.as_slice() {
            b"Do" => op.last_name().map(|n| (Category::XObject, n)),
            b"Tf" => op.name_back(1).map(|n| (Category::Font, n)),
            b"gs" => op.last_name().map(|n| (Category::ExtGState, n)),
            b"scn" | b"SCN" => op.last_name().map(|n| (Category::Pattern, n)),
            _ => None,
        }
    }
}

/// A form's own resources, in the categories it may look a name up in its caller's for.
struct OwnResources {
    xobject: Rc<Dict>,
    font: Rc<Dict>,
    extgstate: Rc<Dict>,
    pattern: Rc<Dict>,
}

impl OwnResources {
    /// The categories of `own`, the resources of the form `form`, which gives them by reference
    /// `given` if it does: each copied once into `copies` for all the forms that give it the same way.
    fn of(doc: &Document, own: &Dict, given: Option<ObjRef>, form: ObjRef, copies: &mut HashMap<CategoryGiven, Rc<Dict>>) -> Self {
        let mut copy = |category: Category, key: &[u8]| {
            let how = match (own.get(key), given) {
                (Some(Object::Ref(entry)), _) => CategoryGiven::Entry(*entry),
                (_, Some(resources)) => CategoryGiven::Resources(resources, category),
                (_, None) => CategoryGiven::Form(form, category),
            };
            copies
                .entry(how)
                .or_insert_with(|| {
                    let copied = res_dict(doc, own, key);
                    #[cfg(test)]
                    OWN_ENTRIES.with(|c| c.set(c.get() + copied.len()));
                    Rc::new(copied)
                })
                .clone()
        };
        OwnResources {
            xobject: copy(Category::XObject, b"XObject"),
            font: copy(Category::Font, b"Font"),
            extgstate: copy(Category::ExtGState, b"ExtGState"),
            pattern: copy(Category::Pattern, b"Pattern"),
        }
    }

    /// The entry these give `name` in `category`, where it is what that category holds (a stream
    /// for an XObject, a dictionary for a font or a graphics state, anything the file defines for a
    /// pattern), as [`super::form_resources`] takes it.
    fn gives(&self, doc: &Document, category: Category, name: &[u8], kinds: &mut Kinds) -> Option<Object> {
        let names = match category {
            Category::XObject => &self.xobject,
            Category::Font => &self.font,
            Category::ExtGState => &self.extgstate,
            Category::Pattern => &self.pattern,
        };
        let entry = names.get(name)?;
        let kind = kind(doc, entry, kinds);
        let holds = match category {
            Category::XObject => kind.stream,
            Category::Font | Category::ExtGState => kind.dict,
            Category::Pattern => kind.defined,
        };
        holds.then(|| entry.clone())
    }
}

/// The scope of a form with resources of its own ([`resource_ops`]): its own names, over its
/// caller's scope (`None`: the resources the cleanup reads), the form, and how many such forms the
/// chain holds.
struct Scoped {
    parent: Option<usize>,
    own: Rc<OwnResources>,
    owner: ObjRef,
    depth: usize,
}

/// Whether the form `r` is on the way to `scope`.
fn on_chain(scopes: &[Scoped], scope: Option<usize>, r: ObjRef) -> bool {
    let mut at = scope;
    while let Some(s) = at.and_then(|i| scopes.get(i)) {
        if s.owner == r {
            return true;
        }
        at = s.parent;
    }
    false
}

/// The entry the innermost of the forms on the way to `scope` gives `name` in `category`, or none
/// when `res`'s names are where it is looked up.
fn given_in(doc: &Document, scopes: &[Scoped], scope: Option<usize>, category: Category, name: &[u8], kinds: &mut Kinds) -> Option<Object> {
    let mut at = scope;
    while let Some(s) = at.and_then(|i| scopes.get(i)) {
        if let Some(entry) = s.own.gives(doc, category, name, kinds) {
            return Some(entry);
        }
        at = s.parent;
    }
    None
}

/// Drop the marked-content properties of `res` that `content`, and what it draws (looked up in
/// `drawn_through` where given, [`resource_ops_with`]), no longer names, returning them.
pub(crate) fn prune_properties(
    doc: &Document,
    budget: &mut DecodeBudget,
    scoped: &mut ScopedReads,
    res: &mut Dict,
    drawn_through: Option<&Dict>,
    content: &[Vec<u8>],
) -> Result<Vec<Object>, CosError> {
    let mut properties = res_dict(doc, res, b"Properties");
    if properties.is_empty() {
        return Ok(Vec::new());
    }
    let mut retired = Vec::new();
    let read = resource_ops_with(doc, budget, scoped, res, drawn_through, content)?;
    let names: Vec<Vec<u8>> = properties.iter().map(|(n, _)| n.to_vec()).collect();
    for name in names {
        if !read.ops.iter().any(|op| matches!(op.op.as_slice(), b"BDC" | b"DP") && op.last_name() == Some(&name)) {
            may_remove(read.partial)?;
            if let Some(value) = properties.remove(&name) {
                retired.push(value);
            }
        }
    }
    if properties.is_empty() {
        res.remove(b"Properties");
    } else {
        res.set(b"Properties".to_vec(), Object::Dict(properties));
    }
    Ok(retired)
}

/// Drop the XObjects of `res` that `content`, and what it draws (looked up in `drawn_through`
/// where given, [`resource_ops_with`]), no longer draws.
pub(super) fn prune_form_resources(
    doc: &Document,
    budget: &mut DecodeBudget,
    scoped: &mut ScopedReads,
    res: &mut Dict,
    drawn_through: Option<&Dict>,
    content: &[Vec<u8>],
) -> Result<(), CosError> {
    let read = resource_ops_with(doc, budget, scoped, res, drawn_through, content)?;
    let mut xo = res_dict(doc, res, b"XObject");
    let names: Vec<Vec<u8>> = xo.iter().map(|(n, _)| n.to_vec()).collect();
    for name in names {
        if !read.ops.iter().any(|op| op.is("Do") && op.last_name() == Some(&name)) {
            may_remove(read.partial)?;
            xo.remove(&name);
        }
    }
    res.set(b"XObject".to_vec(), Object::Dict(xo));
    Ok(())
}

/// The font, writing direction and size an ExtGState's `/Font [font size]` entry sets, as `Tf`
/// would (ISO 32000-2 §8.4.5); `None` when `name` sets no font. The metrics are cached with the
/// fonts `Tf` names, under a key no resource name can take (a name cannot hold a null byte).
pub(super) fn extgstate_font(
    doc: &Document,
    scope: &mut Scope<'_>,
    ext_gstates: &Dict,
    name: &[u8],
    fallback: &Rc<Metrics>,
) -> Option<(Rc<Metrics>, bool, f64)> {
    let state = doc.resolve(ext_gstates.get(name)?).as_dict().cloned()?;
    let entry = doc.resolve(state.get(b"Font")?).as_array().cloned()?;
    let [font, size] = entry.as_slice() else { return None };
    let size = doc.resolve(size).as_f64()?;
    let font_dict = doc.resolve(font).as_dict().cloned();
    let vertical = font_dict.as_ref().is_some_and(|d| is_vertical(doc, d));
    let key = [b"\0".as_slice(), name].concat();
    let metrics = match scope.fonts.get(&key) {
        Some(m) => m.clone(),
        None => {
            let m = font_dict.as_ref().map(|d| Rc::new(Metrics::from_dict(doc, d))).unwrap_or_else(|| fallback.clone());
            scope.fonts.insert(key, m.clone());
            m
        }
    };
    Some((metrics, vertical, size))
}

/// Drop every `/Pattern` entry this scope saw selected but no surviving painting operator drew
/// with. The `scn` that selected it stays in the content, so the page names a pattern its
/// resources no longer hold; nothing surviving paints with that colour, and
/// `a_page_whose_pruned_pattern_is_still_selected_renders` pins that the page still renders and
/// that operators after the stale selector still draw. A pattern's cell is a content stream of its own which
/// this pass does not enter, so a pattern whose only painting was redacted would otherwise keep
/// its drawing -- text included -- in the saved file (#3647). A pattern some surviving paint
/// still uses, or one the scope never saw, is left alone.
/// `/Pattern` may be an indirect object like any other value, so it is resolved: reading the
/// reference itself as a dictionary found none and pruned nothing, leaving the cell in the file.
/// What is written back is a dictionary of this page's own, so pruning never reaches through a
/// `/Pattern` dictionary another page shares. The pattern objects dropped are added to `pruned`,
/// for [`unlink_pruned_patterns`].
pub(crate) fn prune_patterns(doc: &Document, res: &mut Dict, seen: &[Vec<u8>], painted: &[Vec<u8>], pruned: &mut Vec<ObjRef>) -> usize {
    let Some(mut pat) = res.get(b"Pattern").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()) else { return 0 };
    // A set, so each name seen costs one look-up however many are painted.
    let painted: HashSet<&[u8]> = painted.iter().map(Vec::as_slice).collect();
    let mut dropped = 0;
    for n in seen {
        if !painted.contains(n.as_slice())
            && let Some(value) = pat.get(n)
        {
            // Every object along the reference chain: a name can lead to a reference-valued
            // object whose value is the cell, and the cell is what must not stay reachable.
            let mut next = value.as_ref();
            for _ in 0..MAX_DEPTH {
                let Some(reference) = next else { break };
                pruned.push(reference);
                next = match &*doc.get(reference) {
                    Object::Ref(r) => Some(*r),
                    _ => None,
                };
            }
            pat.remove(n);
            dropped += 1;
        }
    }
    if dropped > 0 {
        if pat.is_empty() {
            res.remove(b"Pattern");
        } else {
            res.set(b"Pattern".to_vec(), Object::Dict(pat));
        }
    }
    dropped
}

/// Unlink the patterns a page's rewrite pruned (`pruned`) from page-tree nodes whose
/// `/Resources` no page inherits any more: a full save keeps whatever Root still reaches, so a
/// `/Pages` node above the page that still names the cell would keep it in the file. That happens
/// when the page's nearest holder was retired but a holder further up, which it shadowed, still
/// names the pattern. A holder some page still inherits from is left alone; whatever stays
/// reachable is checked by `pattern_guard`, which refuses a retained cell it cannot prove safe.
/// When any node's `/Parent` disagrees with where the `/Kids` tree puts it (or the root has
/// one), which holder a page inherits from is ambiguous and a node may be reached only through
/// `/Parent`, so nothing is unlinked and the guard, which follows `/Parent` too, decides.
pub(crate) fn unlink_pruned_patterns(doc: &mut Document, pruned: &[ObjRef]) -> Result<(), RedactError> {
    if pruned.is_empty() {
        return Ok(());
    }
    let pruned: HashSet<ObjRef> = pruned.iter().copied().collect();
    let Some(top) = doc.root().and_then(|root| doc.get(root).as_dict().and_then(|d| d.reference(b"Pages"))) else { return Ok(()) };
    // Every node with a /Resources of its own, and whether any page inherits from it (it is the
    // nearest holder above a page, or the page itself). Cycles are cut as in `unlink_doomed`.
    let mut holders: Vec<(ObjRef, bool)> = Vec::new();
    let mut seen = HashSet::new();
    let mut consistent = true;
    let mut stack: Vec<(ObjRef, Option<ObjRef>, Option<usize>)> = vec![(top, None, None)];
    seen.insert(top);
    while let Some((node, parent, mut holder)) = stack.pop() {
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        // Every node's /Parent must be the node /Kids reached it from, and the root has none.
        // A node reached only through a /Parent link is then impossible here; when the links
        // disagree, the guard inspects every node they reach instead. A /Parent that is not a
        // reference (a direct dictionary, written inline by a full save) never agrees.
        let agrees = match d.get(b"Parent") {
            None => parent.is_none(),
            Some(Object::Ref(r)) => Some(*r) == parent,
            Some(_) => false,
        };
        if !agrees {
            consistent = false;
            break;
        }
        if d.contains(b"Resources") {
            holders.push((node, false));
            holder = Some(holders.len() - 1);
        }
        let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
        if is_pages {
            // Borrowed, not copied: a /Kids array can be shared by many hostile nodes.
            let kids_object = d.get(b"Kids").map(|k| doc.resolve(k));
            for kid in kids_object.as_deref().and_then(Object::as_array).into_iter().flatten().filter_map(Object::as_ref) {
                // A node reached twice (a shared /Kids entry or a cycle) has a /Parent that agrees
                // with at most one of the nodes above it.
                if !seen.insert(kid) || seen.len() > 1_000_000 {
                    consistent = false;
                    break;
                }
                stack.push((kid, Some(node), holder));
            }
            if !consistent {
                break;
            }
        } else if let Some(h) = holder.and_then(|i| holders.get_mut(i)) {
            h.1 = true;
        }
    }
    if !consistent {
        return Ok(());
    }
    for (node, inherited) in holders {
        if inherited {
            continue;
        }
        let Some(mut res) = doc.get(node).as_dict().and_then(|d| d.get(b"Resources")).and_then(|r| doc.resolve(r).as_dict().cloned()) else {
            continue;
        };
        let mut pat = res_dict(doc, &res, b"Pattern");
        let names: Vec<Vec<u8>> = pat.iter().filter(|(_, v)| v.as_ref().is_some_and(|r| pruned.contains(&r))).map(|(n, _)| n.to_vec()).collect();
        if names.is_empty() {
            continue;
        }
        for n in &names {
            pat.remove(n);
        }
        if pat.is_empty() {
            res.remove(b"Pattern");
        } else {
            res.set(b"Pattern".to_vec(), Object::Dict(pat));
        }
        // A private copy on this node: the dictionary object may be shared with other nodes.
        doc.update_dict(node, |d| d.set(b"Resources".to_vec(), Object::Dict(res)))?;
    }
    Ok(())
}
