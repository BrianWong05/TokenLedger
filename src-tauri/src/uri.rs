// Percent-decoding and `file://` handling, shared by the adapters that read a
// workspace path out of a Source Artifact and by the export companion, which
// reads the same kind of URI off the wire.

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// A `file://` URI as a local path, percent-decoded.
///
/// `file:///Users/me` → `/Users/me`, but `file:///C:/code` → `C:/code`: the URI
/// form carries a leading slash before a Windows drive letter that is not part
/// of the path. A UNC `file://server/share` names no local path and yields
/// `None` rather than something that merely looks like one, as does any other
/// scheme.
pub fn file_uri_to_path(uri: &str) -> Option<String> {
    let path = uri.strip_prefix("file://")?;
    let path = match path.strip_prefix('/') {
        Some(rest) if has_drive_letter(rest) => rest,
        _ => path,
    };
    (path.starts_with('/') || has_drive_letter(path)).then(|| percent_decode(path))
}

fn has_drive_letter(path: &str) -> bool {
    let mut chars = path.chars();
    matches!((chars.next(), chars.next()), (Some(c), Some(':')) if c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_escapes_and_leaves_the_rest_alone() {
        assert_eq!(percent_decode("My%20Code"), "My Code");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("100%"), "100%"); // truncated escape, kept verbatim
        assert_eq!(percent_decode("%zz"), "%zz"); // not hex
        assert_eq!(percent_decode("caf%C3%A9"), "café"); // multi-byte UTF-8
    }

    #[test]
    fn posix_uris_keep_their_leading_slash() {
        assert_eq!(file_uri_to_path("file:///Users/me/My%20Code").as_deref(), Some("/Users/me/My Code"));
    }

    #[test]
    fn windows_uris_lose_the_slash_that_precedes_the_drive() {
        assert_eq!(file_uri_to_path("file:///C:/code/app").as_deref(), Some("C:/code/app"));
        assert_eq!(file_uri_to_path("file:///c:/Users/me%20x").as_deref(), Some("c:/Users/me x"));
    }

    #[test]
    fn anything_that_is_not_a_local_path_is_none() {
        assert_eq!(file_uri_to_path("file://server/share"), None); // UNC
        assert_eq!(file_uri_to_path("https://example.test/x"), None);
        assert_eq!(file_uri_to_path("/already/a/path"), None);
    }
}
