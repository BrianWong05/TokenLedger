// Minimal protobuf wire-format reader, shared by the Antigravity adapter (which
// decodes `gen_metadata` blobs) and the export companion (which decodes gRPC
// responses off the same server). Neither Source publishes a `.proto`, so
// fields are addressed by number and nothing is generated. Malformed input
// degrades to `None`/empty, never a panic.
//
// Wire types: 0 varint, 1 fixed64, 2 length-delimited, 5 fixed32. Groups (3/4)
// are obsolete and end the walk.

pub fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    while *pos < buf.len() {
        let byte = buf[*pos];
        *pos += 1;
        if shift >= 64 {
            return None;
        }
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

/// Visit every top-level field, stopping as soon as `visit` yields a value.
/// `visit` receives `(field number, wire type, payload, varint value)`; the
/// payload is empty for varints and the varint value is 0 for payloads.
fn walk<'a, T>(
    buf: &'a [u8],
    mut visit: impl FnMut(u64, u8, &'a [u8], u64) -> Option<T>,
) -> Option<T> {
    let mut pos = 0usize;
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let (field, wire) = (key >> 3, (key & 7) as u8);
        match wire {
            0 => {
                let value = read_varint(buf, &mut pos)?;
                if let Some(found) = visit(field, 0, &[], value) {
                    return Some(found);
                }
            }
            1 => pos = pos.checked_add(8)?,
            2 => {
                let len = usize::try_from(read_varint(buf, &mut pos)?).ok()?;
                let end = pos.checked_add(len)?;
                if end > buf.len() {
                    return None;
                }
                if let Some(found) = visit(field, 2, &buf[pos..end], 0) {
                    return Some(found);
                }
                pos = end;
            }
            5 => pos = pos.checked_add(4)?,
            _ => return None,
        }
    }
    None
}

/// First varint field with this number.
pub fn varint_field(buf: &[u8], field_no: u64) -> Option<u64> {
    walk(buf, |field, wire, _, value| (field == field_no && wire == 0).then_some(value))
}

/// First length-delimited field with this number — a nested message or bytes.
pub fn message_field(buf: &[u8], field_no: u64) -> Option<&[u8]> {
    walk(buf, |field, wire, bytes, _| (field == field_no && wire == 2).then_some(bytes))
}

/// Every length-delimited field with this number, in wire order — a `repeated`
/// field. `message_field` returns only the first, which silently drops the rest.
pub fn message_fields(buf: &[u8], field_no: u64) -> Vec<&[u8]> {
    let mut found = Vec::new();
    walk::<()>(buf, |field, wire, bytes, _| {
        if field == field_no && wire == 2 {
            found.push(bytes);
        }
        None
    });
    found
}

pub fn string_field(buf: &[u8], field_no: u64) -> Option<&str> {
    message_field(buf, field_no).and_then(|bytes| std::str::from_utf8(bytes).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint_bytes(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn len_field(no: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = varint_bytes((no << 3) | 2);
        out.extend(varint_bytes(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    fn var_field(no: u64, v: u64) -> Vec<u8> {
        let mut out = varint_bytes(no << 3);
        out.extend(varint_bytes(v));
        out
    }

    #[test]
    fn reads_fields_by_number_and_wire_type() {
        let mut buf = len_field(1, b"one");
        buf.extend(var_field(2, 300)); // multi-byte varint
        buf.extend(len_field(3, b"three"));
        assert_eq!(string_field(&buf, 1), Some("one"));
        assert_eq!(varint_field(&buf, 2), Some(300));
        assert_eq!(string_field(&buf, 3), Some("three"));
        // A number that is present but the wrong wire type is not a match.
        assert_eq!(varint_field(&buf, 1), None);
        assert_eq!(message_field(&buf, 2), None);
        assert_eq!(message_field(&buf, 9), None);
    }

    #[test]
    fn repeated_fields_are_all_returned_in_order() {
        let mut buf = len_field(1, b"a");
        buf.extend(len_field(1, b"b"));
        buf.extend(len_field(2, b"other"));
        buf.extend(len_field(1, b"c"));
        assert_eq!(message_fields(&buf, 1), vec![b"a".as_ref(), b"b".as_ref(), b"c".as_ref()]);
        // The single-value accessor keeps only the first, which is why the
        // repeated one exists at all.
        assert_eq!(message_field(&buf, 1), Some(b"a".as_ref()));
        assert!(message_fields(&buf, 7).is_empty());
    }

    #[test]
    fn fixed_width_fields_are_skipped_not_misread() {
        let mut buf = varint_bytes((1 << 3) | 5); // fixed32
        buf.extend_from_slice(&[1, 2, 3, 4]);
        buf.extend(varint_bytes((2 << 3) | 1)); // fixed64
        buf.extend_from_slice(&[0; 8]);
        buf.extend(var_field(3, 7));
        assert_eq!(varint_field(&buf, 3), Some(7), "the walk must step over 32/64-bit fields");
    }

    #[test]
    fn malformed_input_yields_nothing_rather_than_panicking() {
        assert_eq!(message_field(&[0x0a, 0x40, b'x'], 1), None); // length past the end
        assert_eq!(varint_field(&[0xff, 0xff], 1), None); // truncated key
        assert!(message_fields(&[0xff, 0xff], 1).is_empty());
        assert_eq!(string_field(&len_field(1, &[0xff, 0xfe]), 1), None); // not UTF-8
        assert_eq!(varint_field(&[], 1), None);
    }
}
