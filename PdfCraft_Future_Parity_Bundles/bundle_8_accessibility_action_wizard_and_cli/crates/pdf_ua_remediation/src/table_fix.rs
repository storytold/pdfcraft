//! Table Remediation Engine (/TH Scope and Header Associations).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableHeaderScope {
    Row,
    Column,
    Both,
}

impl TableHeaderScope {
    pub fn to_pdf_name(&self) -> &'static str {
        match self {
            Self::Row => "/Row",
            Self::Column => "/Column",
            Self::Both => "/Both",
        }
    }
}

pub struct TableRemediator;

impl TableRemediator {
    /// Injects `/Scope` attribute into `/TH` table header cell dictionary.
    pub fn remediate_header_cell(th_dict_str: &str, scope: TableHeaderScope) -> String {
        if th_dict_str.contains("/Scope") {
            return th_dict_str.to_string(); // Already specified
        }

        let scope_attr = format!(" /A << /O /Table /Scope {} >>", scope.to_pdf_name());
        if let Some(pos) = th_dict_str.rfind(">>") {
            format!("{}{}{}", &th_dict_str[..pos], scope_attr, &th_dict_str[pos..])
        } else {
            format!("<< /S /TH {} >>", scope_attr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_th_scope_injection() {
        let raw_th = "<< /Type /StructElem /S /TH /P 5 0 R >>";
        let fixed = TableRemediator::remediate_header_cell(raw_th, TableHeaderScope::Column);
        assert!(fixed.contains("/Scope /Column"));
    }
}
