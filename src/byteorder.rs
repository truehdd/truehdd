pub trait WriteBytesLe {
    fn write_le(&self, dst: &mut Vec<u8>);
}

pub trait WriteBytesBe {
    fn write_be(&self, dst: &mut Vec<u8>);
}

macro_rules! impl_num_le_be {
    ($($t:ty),+) => { $(
        impl WriteBytesLe for $t { #[inline] fn write_le(&self, dst: &mut Vec<u8>) { dst.extend_from_slice(&self.to_le_bytes()); }}
        impl WriteBytesBe for $t { #[inline] fn write_be(&self, dst: &mut Vec<u8>) { dst.extend_from_slice(&self.to_be_bytes()); }}
    )+ }
}

impl_num_le_be!(u8, i8, u16, i16, u32, i32, u64, i64, f32, f64);

#[macro_export]
macro_rules! impl_collection {
    ($trait:ident, $method:ident) => {
        impl<T: $trait> $trait for Vec<T> {
            #[inline]
            fn $method(&self, dst: &mut Vec<u8>) {
                self.iter().for_each(|item| item.$method(dst));
            }
        }
        impl<T: $trait, const N: usize> $trait for [T; N] {
            #[inline]
            fn $method(&self, dst: &mut Vec<u8>) {
                self.iter().for_each(|item| item.$method(dst));
            }
        }
    };
}

impl_collection!(WriteBytesLe, write_le);
impl_collection!(WriteBytesBe, write_be);

#[macro_export]
macro_rules! impl_u32_enum {
    ($t:ty) => {
        impl WriteBytesLe for $t {
            fn write_le(&self, dst: &mut Vec<u8>) {
                dst.extend_from_slice(&(*self as u32).to_le_bytes())
            }
        }
        impl WriteBytesBe for $t {
            fn write_be(&self, dst: &mut Vec<u8>) {
                dst.extend_from_slice(&(*self as u32).to_be_bytes())
            }
        }
    };
}

#[macro_export]
macro_rules! join_bytes_le {
    ( $($value:expr),+ $(,)? ) => {{
        let mut vec = Vec::<u8>::new();
        $( $value.write_le(&mut vec); )+
        vec
    }};
}

#[macro_export]
macro_rules! join_bytes_be {
    ( $($value:expr),+ $(,)? ) => {{
        let mut vec = Vec::<u8>::new();
        $( $value.write_be(&mut vec); )+
        vec
    }};
}

#[allow(unused_imports)]
pub use {join_bytes_be, join_bytes_le};

#[cfg(test)]
mod tests {
    use crate::byteorder::{WriteBytesBe, WriteBytesLe};
    use bytes_macro::ToBytes;

    #[derive(ToBytes)]
    struct Mini {
        a: u16,
        b: u32,
        guid: [u8; 4],
    }

    #[test]
    fn to_bytes_roundtrip() {
        let s = Mini {
            a: 0x1234,
            b: 0xABCDEF01,
            guid: *b"TEST",
        };

        let vec_le = &mut Vec::new();
        let vec_be = &mut Vec::new();

        s.write_le(vec_le);
        s.write_be(vec_be);

        let expected_le = [0x34, 0x12, 0x01, 0xEF, 0xCD, 0xAB, b'T', b'E', b'S', b'T'];
        let expected_be = [0x12, 0x34, 0xAB, 0xCD, 0xEF, 0x01, b'T', b'E', b'S', b'T'];

        assert_eq!(&vec_le[..], &expected_le);
        assert_eq!(&vec_be[..], &expected_be);
    }
}
