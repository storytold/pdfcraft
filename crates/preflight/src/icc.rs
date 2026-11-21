//! PDF/A output-intent policy stays here; ICC v2 bytes come from craft-cms.
//! Keep v2.1 rather than substituting the shared v4 built-in profile.

pub fn srgb() -> Vec<u8> {
    craft_cms_v2::builtin::srgb_v2()
}

#[cfg(test)]
#[path = "icc_legacy_test.rs"]
mod legacy;

#[cfg(test)]
mod tests {
    #[test]
    fn shared_profile_preserves_original_bytes() {
        assert_eq!(super::srgb(), super::legacy::srgb());
    }
}
