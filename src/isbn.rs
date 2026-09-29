//! ISBN normalization. The same book can be requested as ISBN-10 or
//! ISBN-13, with or without hyphens: all these spellings share one cache
//! entry and one provider lookup, through their canonical ISBN-13 form.

/// Strip hyphens/spaces and check the result looks like an ISBN-10/13.
pub fn normalize(id: &str) -> Option<String> {
    let isbn: String = id
        .chars()
        .filter(|c| *c != '-' && *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if !isbn.is_ascii() {
        return None;
    }
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let valid = match isbn.len() {
        13 => digits(&isbn),
        10 => digits(&isbn[..9]) && (digits(&isbn[9..]) || &isbn[9..] == "X"),
        _ => false,
    };
    valid.then_some(isbn)
}

/// ISBN-13 form of an ISBN-10 or ISBN-13, `None` if `id` isn't ISBN-shaped.
pub fn to_isbn13(id: &str) -> Option<String> {
    let isbn = normalize(id)?;
    if isbn.len() == 13 {
        return Some(isbn);
    }
    let core = format!("978{}", &isbn[..9]);
    let sum: u32 = core
        .bytes()
        .enumerate()
        .map(|(i, b)| u32::from(b - b'0') * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    Some(format!("{core}{}", (10 - sum % 10) % 10))
}

/// Key under which `id` is cached and looked up: its ISBN-13 form when it is
/// an ISBN, the ID as is otherwise.
pub fn canonical(id: &str) -> String {
    to_isbn13(id).unwrap_or_else(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_isbns() {
        assert_eq!(normalize("978-0-563-53319-1").as_deref(), Some("9780563533191"));
        assert_eq!(normalize("275403143x").as_deref(), Some("275403143X"));
        assert_eq!(normalize("2847342257").as_deref(), Some("2847342257"));
    }

    #[test]
    fn rejects_non_isbns() {
        assert_eq!(normalize("12345"), None);
        assert_eq!(normalize("97805635331X1"), None);
        assert_eq!(normalize("isbn:(*)"), None);
        assert_eq!(normalize("é12345678"), None);
        assert_eq!(normalize("12345678é"), None);
    }

    #[test]
    fn converts_isbn10_to_isbn13() {
        assert_eq!(to_isbn13("2847342257").as_deref(), Some("9782847342253"));
        assert_eq!(to_isbn13("0563533196").as_deref(), Some("9780563533191"));
        assert_eq!(to_isbn13("0-8214-1749-5").as_deref(), Some("9780821417492"));
        // Check digit X, and a result whose check digit is 0.
        assert_eq!(to_isbn13("275403143X").as_deref(), Some("9782754031431"));
        assert_eq!(to_isbn13("2070360024").as_deref(), Some("9782070360024"));
        // Indonesian ISBN-10 starting with 979: still a 978 ISBN-13.
        assert_eq!(to_isbn13("9791600295").as_deref(), Some("9789791600293"));
    }

    #[test]
    fn canonical_keys() {
        assert_eq!(canonical("978-2-84734-225-3"), "9782847342253");
        assert_eq!(canonical("2847342257"), "9782847342253");
        assert_eq!(canonical("9798885795692"), "9798885795692");
        assert_eq!(canonical("B0CJHSX7Q4"), "B0CJHSX7Q4");
        assert_eq!(canonical("ocm12345"), "ocm12345");
    }
}
