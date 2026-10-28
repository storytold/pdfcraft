//! Document Security Store (/DSS) and Validation Related Information (/VRI) per ETSI EN 319 142.

use std::collections::HashMap;
use std::fmt::Write;

pub struct DocumentSecurityStoreBuilder {
    certs: Vec<Vec<u8>>,
    ocsps: Vec<Vec<u8>>,
    crls: Vec<Vec<u8>>,
    vri_entries: HashMap<String, (Vec<usize>, Vec<usize>, Vec<usize>)>, // SigHash -> (cert_indices, ocsp_indices, crl_indices)
}

impl Default for DocumentSecurityStoreBuilder {
    fn default() -> Self {
        Self {
            certs: Vec::new(),
            ocsps: Vec::new(),
            crls: Vec::new(),
            vri_entries: HashMap::new(),
        }
    }
}

impl DocumentSecurityStoreBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_certificate(&mut self, der_bytes: Vec<u8>) -> usize {
        let idx = self.certs.len();
        self.certs.push(der_bytes);
        idx
    }

    pub fn add_ocsp_response(&mut self, der_bytes: Vec<u8>) -> usize {
        let idx = self.ocsps.len();
        self.ocsps.push(der_bytes);
        idx
    }

    pub fn add_crl(&mut self, der_bytes: Vec<u8>) -> usize {
        let idx = self.crls.len();
        self.crls.push(der_bytes);
        idx
    }

    pub fn register_vri_entry(
        &mut self,
        sig_hash_hex: &str,
        cert_idx: usize,
        ocsp_idx: usize,
    ) {
        self.vri_entries.insert(
            sig_hash_hex.to_uppercase(),
            (vec![cert_idx], vec![ocsp_idx], Vec::new()),
        );
    }

    /// Serializes `/DSS` dictionary entries into PDF catalog format.
    pub fn serialize_dss_dict(&self, base_object_id: u32) -> String {
        let mut out = String::new();
        writeln!(&mut out, "<< /Type /DSS").unwrap();

        if !self.certs.is_empty() {
            write!(&mut out, "  /Certs [").unwrap();
            for i in 0..self.certs.len() {
                write!(&mut out, " {} 0 R", base_object_id + i as u32).unwrap();
            }
            writeln!(&mut out, " ]").unwrap();
        }

        if !self.ocsps.is_empty() {
            write!(&mut out, "  /OCSPs [").unwrap();
            let ocsp_base = base_object_id + self.certs.len() as u32;
            for i in 0..self.ocsps.len() {
                write!(&mut out, " {} 0 R", ocsp_base + i as u32).unwrap();
            }
            writeln!(&mut out, " ]").unwrap();
        }

        if !self.vri_entries.is_empty() {
            writeln!(&mut out, "  /VRI <<").unwrap();
            for (hash, _) in &self.vri_entries {
                writeln!(&mut out, "    /{} << /Type /VRI >>", hash).unwrap();
            }
            writeln!(&mut out, "  >>").unwrap();
        }

        write!(&mut out, ">>").unwrap();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dss_dictionary_builder() {
        let mut dss = DocumentSecurityStoreBuilder::new();
        let c_idx = dss.add_certificate(vec![0x30, 0x82, 0x01]);
        let o_idx = dss.add_ocsp_response(vec![0x30, 0x82, 0x02]);
        dss.register_vri_entry("E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855", c_idx, o_idx);

        let dict_str = dss.serialize_dss_dict(10);
        assert!(dict_str.contains("/Type /DSS"));
        assert!(dict_str.contains("/Certs [ 10 0 R ]"));
        assert!(dict_str.contains("/OCSPs [ 11 0 R ]"));
        assert!(dict_str.contains("/VRI <<"));
    }
}
