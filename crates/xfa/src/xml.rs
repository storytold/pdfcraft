//! Namespace-aware XFA DOM delegated to the shared implementation.
pub(crate) use craft_xfa::xml::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_dom_keeps_depth_policy_and_rejects_duplicate_attributes() {
        assert_eq!(crate::MAX_XML_DEPTH, craft_xfa::MAX_XML_DEPTH);
        assert!(Document::parse("<r a='1' a='2'/>").is_err());
        assert!(Document::parse("<r xmlns:a='u' xmlns:b='u' a:x='1' b:x='2'/>").is_err());
        let doc = Document::parse("<r xmlns='u'><field>value</field></r>").unwrap();
        let field = doc.root_element().children().next().unwrap();
        assert_eq!(field.tag_name().namespace(), Some("u"));
        assert_eq!(field.text(), Some("value"));
    }
}
