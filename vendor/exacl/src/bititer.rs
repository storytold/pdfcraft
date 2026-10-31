//! Implements a generic bit iterator.
//!
//! Works with built-in integer types or bitflags. You just have to implement
//! the `BitIterable` trait.

use std::ops::BitXorAssign;

pub trait BitIterable: Copy + BitXorAssign {
    fn lsb(self) -> Option<Self>;
    fn msb(self) -> Option<Self>;
}

pub struct BitIter<T: BitIterable>(pub T);

impl<T: BitIterable> Iterator for BitIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.0.lsb().map(|bit| {
            self.0 ^= bit;
            bit
        })
    }
}

impl<T: BitIterable> DoubleEndedIterator for BitIter<T> {
    fn next_back(&mut self) -> Option<T> {
        self.0.msb().map(|bit| {
            self.0 ^= bit;
            bit
        })
    }
}

////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod bititer_tests {
    #![allow(clippy::unreadable_literal)]

    use super::*;
    use bitflags::bitflags;

    impl BitIterable for u32 {
        fn lsb(self) -> Option<Self> {
            if self == 0 {
                return None;
            }
            Some(1 << self.trailing_zeros())
        }

        fn msb(self) -> Option<Self> {
            if self == 0 {
                return None;
            }
            Some(1 << self.ilog2())
        }
    }

    #[test]
    fn test_bititer_u32() {
        assert!(BitIter(0).next().is_none());

        let v = BitIter(1).collect::<Vec<u32>>();
        assert_eq!(v, vec![1]);

        let v = BitIter(1 << 31).collect::<Vec<u32>>();
        assert_eq!(v, vec![1 << 31]);

        let v = BitIter(2 + 4 + 16 + 64).collect::<Vec<u32>>();
        assert_eq!(v, vec![2, 4, 16, 64]);

        let v = BitIter(u32::MAX).collect::<Vec<u32>>();
        assert_eq!(v.len(), 32);
        assert_eq!(
            v,
            vec![
                1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
                131072, 262144, 524288, 1048576, 2097152, 4194304, 8388608, 16777216, 33554432,
                67108864, 134217728, 268435456, 536870912, 1073741824, 2147483648
            ]
        );
    }

    #[test]
    fn test_bititer_u32_rev() {
        assert!(BitIter(0).next_back().is_none());

        let v = BitIter(1).rev().collect::<Vec<u32>>();
        assert_eq!(v, vec![1]);

        let v = BitIter(1 << 31).rev().collect::<Vec<u32>>();
        assert_eq!(v, vec![1 << 31]);

        let v = BitIter(2 + 4 + 16 + 64).rev().collect::<Vec<u32>>();
        assert_eq!(v, vec![64, 16, 4, 2]);

        let v = BitIter(u32::MAX).rev().collect::<Vec<u32>>();
        assert_eq!(v.len(), 32);
        assert_eq!(
            v,
            vec![
                2147483648, 1073741824, 536870912, 268435456, 134217728, 67108864, 33554432,
                16777216, 8388608, 4194304, 2097152, 1048576, 524288, 262144, 131072, 65536, 32768,
                16384, 8192, 4096, 2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1
            ]
        );
    }

    bitflags! {
        #[derive(Copy, Clone, Debug, Default, PartialEq)]
        struct TestBit: u32 {
            const BIT1 = 1 << 0;
            const BIT2 = 1 << 1;
            const BIT3 = 1 << 5;
        }
    }

    impl BitIterable for TestBit {
        fn lsb(self) -> Option<Self> {
            if self.is_empty() {
                return None;
            }
            let low_bit = 1 << self.bits().trailing_zeros();
            Some(TestBit::from_bits_retain(low_bit))
        }

        fn msb(self) -> Option<Self> {
            #[allow(clippy::cast_possible_truncation)]
            const MAX_BITS: u32 = 8 * std::mem::size_of::<TestBit>() as u32 - 1;
            if self.is_empty() {
                return None;
            }
            let high_bit = 1 << (MAX_BITS - self.bits().leading_zeros());
            Some(TestBit::from_bits_retain(high_bit))
        }
    }

    #[test]
    fn test_bititer_bitflags() {
        let bits = TestBit::BIT1 | TestBit::BIT2 | TestBit::BIT3;

        let v = BitIter(TestBit::empty()).collect::<Vec<TestBit>>();
        assert_eq!(v, vec![]);

        let v = BitIter(bits.bits()).collect::<Vec<u32>>();
        assert_eq!(v, vec![1, 2, 32]);

        let v = BitIter(bits).collect::<Vec<TestBit>>();
        assert_eq!(
            v,
            vec![
                TestBit::from_bits_retain(1),
                TestBit::from_bits_retain(2),
                TestBit::from_bits_retain(32)
            ]
        );
    }

    #[test]
    fn test_bititer_bitflags_rev() {
        let bits = TestBit::BIT1 | TestBit::BIT2 | TestBit::BIT3;

        let v = BitIter(TestBit::empty()).rev().collect::<Vec<TestBit>>();
        assert_eq!(v, vec![]);

        let v = BitIter(bits.bits()).rev().collect::<Vec<u32>>();
        assert_eq!(v, vec![32, 2, 1]);

        let v = BitIter(bits).rev().collect::<Vec<TestBit>>();
        assert_eq!(
            v,
            vec![
                TestBit::from_bits_retain(32),
                TestBit::from_bits_retain(2),
                TestBit::from_bits_retain(1)
            ]
        );
    }
}
