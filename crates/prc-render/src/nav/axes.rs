//! Which of the model's axes points up, and which way the Front view looks.

/// One signed model axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// +x.
    PosX,
    /// +y.
    PosY,
    /// +z.
    PosZ,
    /// -x.
    NegX,
    /// -y.
    NegY,
    /// -z.
    NegZ,
}

impl Axis {
    /// Every axis, in [`Axis::token`] order `+x +y +z -x -y -z`.
    pub const ALL: [Axis; 6] = [Axis::PosX, Axis::PosY, Axis::PosZ, Axis::NegX, Axis::NegY, Axis::NegZ];

    /// The unit vector it names.
    #[must_use]
    pub fn vector(self) -> [f64; 3] {
        match self {
            Axis::PosX => [1.0, 0.0, 0.0],
            Axis::PosY => [0.0, 1.0, 0.0],
            Axis::PosZ => [0.0, 0.0, 1.0],
            Axis::NegX => [-1.0, 0.0, 0.0],
            Axis::NegY => [0.0, -1.0, 0.0],
            Axis::NegZ => [0.0, 0.0, -1.0],
        }
    }

    /// Its token: `+x` … `-z`.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Axis::PosX => "+x",
            Axis::PosY => "+y",
            Axis::PosZ => "+z",
            Axis::NegX => "-x",
            Axis::NegY => "-y",
            Axis::NegZ => "-z",
        }
    }

    /// The axis whose [`Axis::token`] is `token`.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Axis> {
        Axis::ALL.into_iter().find(|a| a.token() == token)
    }

    /// Whether `other` lies along a different line through the origin.
    #[must_use]
    pub fn perpendicular(self, other: Axis) -> bool {
        self.line() != other.line()
    }

    /// 0, 1 or 2 for the x, y or z line.
    fn line(self) -> u8 {
        match self {
            Axis::PosX | Axis::NegX => 0,
            Axis::PosY | Axis::NegY => 1,
            Axis::PosZ | Axis::NegZ => 2,
        }
    }

    /// The Front view's look when this axis is up: along +y for z or x up (the CAD convention),
    /// along -z for y up (the convention of y-up exporters).
    fn default_front(self) -> Axis {
        match self {
            Axis::PosY | Axis::NegY => Axis::NegZ,
            _ => Axis::PosY,
        }
    }
}

/// The up axis and the Front view's look direction; always perpendicular.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Upright {
    up: Axis,
    front: Axis,
}

impl Default for Upright {
    /// z up, Front looking along +y.
    fn default() -> Self {
        Upright { up: Axis::PosZ, front: Axis::PosY }
    }
}

impl Upright {
    /// The up axis.
    #[must_use]
    pub fn up(self) -> Axis {
        self.up
    }

    /// The Front view's look direction.
    #[must_use]
    pub fn front(self) -> Axis {
        self.front
    }

    /// `up` as the up axis, keeping the Front look when it is still perpendicular, else that
    /// axis's conventional one.
    #[must_use]
    pub fn with_up(self, up: Axis) -> Upright {
        let front = if up.perpendicular(self.front) { self.front } else { up.default_front() };
        Upright { up, front }
    }

    /// `front` as the Front view's look, or unchanged when it lies along up.
    #[must_use]
    pub fn with_front(self, front: Axis) -> Upright {
        if self.up.perpendicular(front) { Upright { front, ..self } } else { self }
    }

    /// The Front looks a choice can offer: the four perpendicular to up.
    pub fn fronts(self) -> impl Iterator<Item = Axis> {
        Axis::ALL.into_iter().filter(move |a| self.up.perpendicular(*a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_up_keeps_a_front_that_still_fits_and_replaces_one_that_does_not() {
        let start = Upright::default();
        assert_eq!(start.with_up(Axis::PosX).front(), Axis::PosY);
        assert_eq!(start.with_up(Axis::PosY).front(), Axis::NegZ);
        assert_eq!(start.with_front(Axis::NegZ), start);
        assert_eq!(start.fronts().count(), 4);
    }

    #[test]
    fn every_axis_is_a_signed_unit_vector_with_a_token_that_reads_back() {
        assert_eq!(Axis::NegY.vector(), [0.0, -1.0, 0.0]);
        assert_eq!(Axis::PosZ.vector(), [0.0, 0.0, 1.0]);
        for a in Axis::ALL {
            assert_eq!(Axis::from_token(a.token()), Some(a));
        }
        assert_eq!(Axis::from_token("z"), None);
    }
}
