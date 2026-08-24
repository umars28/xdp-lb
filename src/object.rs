#[repr(align(8))]
struct Aligned<T: ?Sized>(T);

static OBJECT: &Aligned<[u8]> = &Aligned(*include_bytes!(env!("BPF_OBJECT")));

pub fn bytes() -> &'static [u8] {
    &OBJECT.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_eight_byte_aligned() {
        assert_eq!(bytes().as_ptr().align_offset(8), 0);
    }

    #[test]
    fn bytes_look_like_an_elf_object() {
        assert_eq!(&bytes()[..4], b"\x7fELF");
    }
}
