//! Feature property bags ([MS-XLSX] 2.1.22, 2.3.9): how Excel stores cell checkboxes.
//!
//! A checkbox is a cell format (`xf`) whose `extLst` carries an `xfComplement i="n"` element
//! ([MS-XLSX] 2.4.99, styles extension `{C7286773-470A-42A8-94C5-96B5CB345126}`). `n` indexes the
//! `MappedFeaturePropertyBags` array of the `XFComplements` bag; that entry is an `XFComplement`
//! bag whose `XFControls` key names an `XFControls` bag, whose `CellControl` key names a
//! `Checkbox` bag. Bag references are zero-based indexes into the part's list of `bag` elements.
//!
//! GridCraft keeps a checkbox as the `checkbox` number format in its model, so reading maps each
//! `xfComplement` that resolves to a `Checkbox` bag onto that format, and writing emits one chain
//! that every checkbox `xf` points at.

use crate::xml::El;

/// Namespace of the part and of the `xfComplement` element.
pub const NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2022/featurepropertybag";
/// Relationship type from the workbook part.
pub const REL: &str = "http://schemas.microsoft.com/office/2022/11/relationships/FeaturePropertyBag";
/// Content type of the part.
pub const CONTENT_TYPE: &str = "application/vnd.ms-excel.featurepropertybag+xml";
/// Part name we write.
pub const PART: &str = "xl/featurePropertyBag/featurePropertyBag.xml";
/// `ext` URI of the `xfComplement` extension on an `xf`.
pub const XF_EXT_URI: &str = "{C7286773-470A-42A8-94C5-96B5CB345126}";

/// The bags for one checkbox cell control, referenced as `xfComplement i="0"`: Checkbox (0),
/// XFControls → 0 (1), XFComplement → 1 (2), XFComplements mapping index 0 → 2 (3). Every
/// reference points to an earlier bag, as [MS-XLSX] 2.6.230 requires. The XFComplements bag is
/// the one `xfComplement` indexes into, and Excel finds it by its `extRef`: without
/// `extRef="XFComplementsMapperExtRef"` it repairs the workbook ("Format from /xl/styles.xml")
/// and drops every checkbox (#2630).
pub fn checkbox_part() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<FeaturePropertyBags xmlns=\"{NS}\" count=\"4\"><bag type=\"Checkbox\"/><bag type=\"XFControls\"><bagId k=\"CellControl\">0</bagId></bag><bag type=\"XFComplement\"><bagId k=\"XFControls\">1</bagId></bag><bag type=\"XFComplements\" extRef=\"XFComplementsMapperExtRef\"><a k=\"MappedFeaturePropertyBags\"><bagId>2</bagId></a></bag></FeaturePropertyBags>"
    )
}

/// The `extLst` that marks an `xf` as a checkbox (complement index 0 of [`checkbox_part`]).
pub fn checkbox_xf_ext() -> String {
    format!("<extLst><ext uri=\"{XF_EXT_URI}\" xmlns:xfpb=\"{NS}\"><xfpb:xfComplement i=\"0\"/></ext></extLst>")
}

/// For each `xfComplement` index, whether it resolves to a Checkbox cell control. Unknown,
/// malformed or out-of-range references simply resolve to `false`.
pub fn checkbox_complements(root: &El) -> Vec<bool> {
    let bags: Vec<&El> = root.kids("bag").collect();
    let bag = |i: Option<u32>, ty: &str| i.and_then(|i| bags.get(i as usize).copied()).filter(|b| b.attr("type") == Some(ty));
    let key = |b: &El, k: &str| b.kids("bagId").find(|e| e.attr("k") == Some(k)).and_then(|e| e.text.trim().parse::<u32>().ok());
    let Some(map) = bags.iter().find(|b| b.attr("type") == Some("XFComplements")) else { return Vec::new() };
    let Some(arr) = map.kids("a").find(|a| a.attr("k") == Some("MappedFeaturePropertyBags")) else { return Vec::new() };
    arr.kids("bagId")
        .take(100_000)
        .map(|e| {
            let complement = bag(e.text.trim().parse::<u32>().ok(), "XFComplement");
            let controls = complement.and_then(|c| bag(key(c, "XFControls"), "XFControls"));
            controls.and_then(|c| bag(key(c, "CellControl"), "Checkbox")).is_some()
        })
        .collect()
}

/// Whether an `xf` element carries an `xfComplement` that resolves to a checkbox.
pub fn xf_is_checkbox(xf: &El, complements: &[bool]) -> bool {
    let Some(ext) = xf.child("extLst") else { return false };
    ext.kids("ext")
        .filter(|e| e.attr("uri").is_some_and(|u| u.eq_ignore_ascii_case(XF_EXT_URI)))
        .filter_map(|e| e.child("xfComplement"))
        .filter_map(|c| c.attr_u32("i"))
        .any(|i| complements.get(i as usize).copied().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_chain_resolves_and_hostile_chains_do_not() {
        let root = crate::xml::parse(checkbox_part().as_bytes()).unwrap_or_default();
        assert_eq!(checkbox_complements(&root), vec![true]);
        // #2630: Excel repairs the file unless the mapping bag carries this extRef.
        let mapper = root.kids("bag").find(|b| b.attr("type") == Some("XFComplements"));
        assert_eq!(mapper.and_then(|b| b.attr("extRef")), Some("XFComplementsMapperExtRef"));
        // Out-of-range, wrong-type and self references resolve to false without panicking.
        let bad = format!(
            "<FeaturePropertyBags xmlns=\"{NS}\"><bag type=\"XFControls\"><bagId k=\"CellControl\">99</bagId></bag><bag type=\"XFComplement\"><bagId k=\"XFControls\">1</bagId></bag><bag type=\"XFComplements\"><a k=\"MappedFeaturePropertyBags\"><bagId>1</bagId><bagId>4294967295</bagId><bagId>x</bagId></a></bag></FeaturePropertyBags>"
        );
        let root = crate::xml::parse(bad.as_bytes()).unwrap_or_default();
        assert_eq!(checkbox_complements(&root), vec![false, false, false]);
    }
}
