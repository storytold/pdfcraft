use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
/// Represents a media size tuple.
pub struct MediaSizeTuple(u32, u32);
impl MediaSizeTuple {
    /// Create a new media size tuple using width and height in micron.
    pub const fn micron(width: u32, height: u32) -> Self {
        MediaSizeTuple(width, height)
    }
    /// Create a new media size tuple using width and height in millimeter.
    pub const fn mm(width: u32, height: u32) -> Self {
        MediaSizeTuple(width * 1000, height * 1000)
    }
    /// Get the width in micron.
    pub const fn width_in_micron(&self) -> u32 {
        self.0
    }
    /// Get the height in micron.
    pub const fn height_in_micron(&self) -> u32 {
        self.1
    }
    /// Set the width in micron.
    pub fn set_width_in_micron(&mut self, width: u32) {
        self.0 = width;
    }
    /// Set the height in micron.
    pub fn set_height_in_micron(&mut self, height: u32) {
        self.1 = height;
    }
    /// Determine if the media is a roll media.
    pub fn is_roll(&self) -> bool {
        // for roll media, either width or height is 0
        self.0 == 0 || self.1 == 0
    }
}

impl fmt::Debug for MediaSizeTuple {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "MediaSizeTuple({}.{:0>4} cm × {}.{:0>4} cm)",
            self.0 / 10000,
            self.0 % 10000,
            self.1 / 10000,
            self.1 % 10000
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_getter_and_setter() {
        let mut size = MediaSizeTuple::mm(210, 297);
        assert_eq!(size.width_in_micron(), 210000);
        assert_eq!(size.height_in_micron(), 297000);
        size.set_width_in_micron(297000);
        size.set_height_in_micron(210000);
        assert_eq!(size.width_in_micron(), 297000);
        assert_eq!(size.height_in_micron(), 210000);
    }

    #[test]
    fn test_debug_fmt() {
        assert_eq!(
            format!("{:?}", MediaSizeTuple::mm(210, 297)),
            "MediaSizeTuple(21.0000 cm × 29.7000 cm)"
        );
        assert_eq!(
            format!("{:?}", MediaSizeTuple::micron(210001, 297001)),
            "MediaSizeTuple(21.0001 cm × 29.7001 cm)"
        );
    }

    #[test]
    fn test_eq() {
        assert_eq!(
            MediaSizeTuple::micron(210000, 297000),
            MediaSizeTuple::mm(210, 297)
        );
    }

    #[test]
    fn test_clone() {
        let size = MediaSizeTuple::mm(210, 297);
        assert_eq!(size, size.clone());
    }

    #[test]
    fn test_hash() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let hash1 = {
            let mut hasher = DefaultHasher::new();
            MediaSizeTuple::mm(210, 297).hash(&mut hasher);
            hasher.finish()
        };
        let hash2 = {
            let mut hasher = DefaultHasher::new();
            MediaSizeTuple::micron(210000, 297000).hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_is_roll() {
        assert_eq!(MediaSizeTuple::mm(210, 297).is_roll(), false);
        assert!(MediaSizeTuple::mm(210, 0).is_roll());
        assert!(MediaSizeTuple::mm(0, 297).is_roll());
    }
}
