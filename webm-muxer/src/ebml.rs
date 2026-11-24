//! EBML encoding (RFC 8794): variable-size integers, element headers and
//! the basic element types, built into small in-memory buffers. Frame
//! payloads never pass through here; the muxer writes them straight to
//! the output.

/// An element data size of "unknown" (RFC 8794, section 6.2), in the
/// 8-octet form. The muxer also uses this form as a placeholder that it
/// overwrites with the real size, so the layout never moves.
pub(crate) const UNKNOWN_SIZE: [u8; 8] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

/// The largest data size an 8-octet VINT can hold (all value bits set is
/// reserved for "unknown").
pub(crate) const MAX_SIZE: u64 = (1 << 56) - 2;

/// The shortest VINT length for a data size. Lengths whose value bits
/// would all be set are skipped, since that pattern means "unknown".
/// `None` when the value doesn't fit in 8 octets.
pub(crate) fn size_len(value: u64) -> Option<usize> {
    (1..=8usize).find(|&len| value < (1u64 << (7 * len)) - 1)
}

/// Appends `value` as a data size VINT of the shortest length.
/// Callers have checked `value <= MAX_SIZE`.
pub(crate) fn push_size(out: &mut Vec<u8>, value: u64) {
    let len = size_len(value).unwrap_or(8);
    push_size_len(out, value, len);
}

/// Appends `value` as a VINT of exactly `len` octets.
pub(crate) fn push_size_len(out: &mut Vec<u8>, value: u64, len: usize) {
    let marked = value | (1u64 << (7 * len));
    let bytes = marked.to_be_bytes();
    out.extend_from_slice(&bytes[8 - len..]);
}

/// `value` as an 8-octet data size VINT.
pub(crate) fn size8(value: u64) -> [u8; 8] {
    let mut out = Vec::with_capacity(8);
    push_size_len(&mut out, value, 8);
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&out);
    bytes
}

/// The number of octets in an element ID (its marker bits included).
pub(crate) fn id_len(id: u32) -> usize {
    if id >= 0x0100_0000 {
        4
    } else if id >= 0x01_0000 {
        3
    } else if id >= 0x0100 {
        2
    } else {
        1
    }
}

/// Appends an element ID.
pub(crate) fn push_id(out: &mut Vec<u8>, id: u32) {
    let len = id_len(id);
    out.extend_from_slice(&id.to_be_bytes()[4 - len..]);
}

/// Appends an element header: the ID and the shortest size VINT.
pub(crate) fn push_header(out: &mut Vec<u8>, id: u32, size: u64) {
    push_id(out, id);
    push_size(out, size);
}

/// The length of an element header for this ID and data size.
pub(crate) fn header_len(id: u32, size: u64) -> u64 {
    (id_len(id) + size_len(size).unwrap_or(8)) as u64
}

/// The shortest big-endian form of an unsigned integer (at least one
/// octet).
fn uint_bytes(value: u64) -> ([u8; 8], usize) {
    let bytes = value.to_be_bytes();
    let skip = (value.leading_zeros() / 8).min(7) as usize;
    (bytes, skip)
}

/// The shortest two's-complement big-endian form of a signed integer.
fn int_bytes(value: i64) -> ([u8; 8], usize) {
    let bytes = value.to_be_bytes();
    let mut skip = 0;
    while skip < 7 {
        let (here, next) = (bytes[skip], bytes[skip + 1]);
        let redundant = (here == 0x00 && next & 0x80 == 0) || (here == 0xFF && next & 0x80 != 0);
        if !redundant {
            break;
        }
        skip += 1;
    }
    (bytes, skip)
}

/// Builds elements into a buffer.
#[derive(Default)]
pub(crate) struct Buf(pub(crate) Vec<u8>);

impl Buf {
    pub(crate) fn new() -> Self {
        Buf(Vec::new())
    }

    pub(crate) fn len(&self) -> u64 {
        self.0.len() as u64
    }

    pub(crate) fn uint(&mut self, id: u32, value: u64) {
        let (bytes, skip) = uint_bytes(value);
        push_header(&mut self.0, id, (8 - skip) as u64);
        self.0.extend_from_slice(&bytes[skip..]);
    }

    /// An unsigned integer in exactly 8 octets, so it can be rewritten in
    /// place later.
    pub(crate) fn uint8(&mut self, id: u32, value: u64) {
        push_header(&mut self.0, id, 8);
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn int(&mut self, id: u32, value: i64) {
        let (bytes, skip) = int_bytes(value);
        push_header(&mut self.0, id, (8 - skip) as u64);
        self.0.extend_from_slice(&bytes[skip..]);
    }

    pub(crate) fn float(&mut self, id: u32, value: f64) {
        push_header(&mut self.0, id, 8);
        self.0.extend_from_slice(&value.to_bits().to_be_bytes());
    }

    pub(crate) fn string(&mut self, id: u32, value: &str) {
        self.binary(id, value.as_bytes());
    }

    pub(crate) fn binary(&mut self, id: u32, value: &[u8]) {
        push_header(&mut self.0, id, value.len() as u64);
        self.0.extend_from_slice(value);
    }

    /// A master element holding what `fill` writes.
    pub(crate) fn master(&mut self, id: u32, fill: impl FnOnce(&mut Buf)) {
        let mut inner = Buf::new();
        fill(&mut inner);
        push_header(&mut self.0, id, inner.len());
        self.0.extend_from_slice(&inner.0);
    }

    /// A Void element exactly `total` octets long, header included.
    /// `total` must be at least 2.
    pub(crate) fn void(&mut self, total: u64) {
        // A 1-octet size covers data up to 126 octets; beyond that, use
        // the 8-octet form so the header length is fixed.
        if total <= 128 {
            let data = total - 2;
            push_id(&mut self.0, crate::ids::VOID);
            push_size_len(&mut self.0, data, 1);
            self.0.resize(self.0.len() + data as usize, 0);
        } else {
            let data = total - 9;
            push_id(&mut self.0, crate::ids::VOID);
            push_size_len(&mut self.0, data, 8);
            self.0.resize(self.0.len() + data as usize, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_the_shortest_form_and_avoid_all_ones() {
        let mut out = Vec::new();
        push_size(&mut out, 0);
        assert_eq!(out, [0x80]);
        out.clear();
        push_size(&mut out, 126);
        assert_eq!(out, [0xFE]);
        out.clear();
        // 127 in one octet would be 0xFF, which means "unknown".
        push_size(&mut out, 127);
        assert_eq!(out, [0x40, 0x7F]);
        out.clear();
        push_size(&mut out, 16_382);
        assert_eq!(out, [0x7F, 0xFE]);
        out.clear();
        push_size(&mut out, 16_383);
        assert_eq!(out, [0x20, 0x3F, 0xFF]);
        assert_eq!(size_len(MAX_SIZE), Some(8));
        assert_eq!(size_len(MAX_SIZE + 1), None);
        assert_eq!(size8(5), [0x01, 0, 0, 0, 0, 0, 0, 5]);
    }

    #[test]
    fn ids_keep_their_marker_bits() {
        let mut out = Vec::new();
        push_id(&mut out, crate::ids::EBML);
        push_id(&mut out, crate::ids::SEEK);
        push_id(&mut out, crate::ids::VOID);
        assert_eq!(out, [0x1A, 0x45, 0xDF, 0xA3, 0x4D, 0xBB, 0xEC]);
    }

    #[test]
    fn integers_are_minimal() {
        let mut b = Buf::new();
        b.uint(0x83, 0);
        b.uint(0x83, 1);
        b.uint(0x83, 256);
        assert_eq!(b.0, [0x83, 0x81, 0, 0x83, 0x81, 1, 0x83, 0x82, 1, 0]);
        let mut b = Buf::new();
        b.int(0xFB, -1);
        b.int(0xFB, -128);
        b.int(0xFB, -129);
        b.int(0xFB, 127);
        b.int(0xFB, 128);
        assert_eq!(
            b.0,
            [0xFB, 0x81, 0xFF, 0xFB, 0x81, 0x80, 0xFB, 0x82, 0xFF, 0x7F, 0xFB, 0x81, 0x7F, 0xFB, 0x82, 0x00, 0x80]
        );
    }

    #[test]
    fn voids_have_the_exact_length() {
        for total in [2u64, 3, 11, 127, 128, 129, 130, 1000] {
            let mut b = Buf::new();
            b.void(total);
            assert_eq!(b.len(), total, "void of {total}");
        }
    }
}
