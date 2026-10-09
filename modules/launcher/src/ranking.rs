/// Score how well `text` matches `query` (case-insensitive).
///
/// Tiers: exact > prefix > word-prefix > substring > subsequence (3+ chars).
/// Shorter texts get a small bonus so tighter matches rank first.
pub fn score(query: &str, text: &str) -> Option<u32> {
    let query = query.trim().to_lowercase();

    if query.is_empty() {
        return None;
    }

    let text = text.to_lowercase();
    let bonus = 99u32.saturating_sub(text.chars().count() as u32);

    let base = if text == query {
        1000
    } else if text.starts_with(&query) {
        800
    } else if text
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(&query))
    {
        600
    } else if text.contains(&query) {
        400
    } else if query.chars().count() >= 3 && is_subsequence(&query, &text) {
        200
    } else {
        return None;
    };

    Some(base + bonus)
}

fn is_subsequence(query: &str, text: &str) -> bool {
    let mut chars = text.chars();

    query.chars().all(|wanted| chars.any(|c| c == wanted))
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn tiers_are_ordered() {
        let exact = score("firefox", "Firefox").unwrap();
        let prefix = score("fire", "Firefox").unwrap();
        let word = score("web", "Firefox Web Browser").unwrap();
        let substring = score("ox", "Firefox").unwrap();
        let subsequence = score("ffx", "Firefox").unwrap();

        assert!(exact > prefix);
        assert!(prefix > word);
        assert!(word > substring);
        assert!(substring > subsequence);
    }

    #[test]
    fn no_match_and_empty_query() {
        assert_eq!(score("zzz", "Firefox"), None);
        assert_eq!(score("   ", "Firefox"), None);
    }

    #[test]
    fn short_queries_do_not_fuzzy_match() {
        assert_eq!(score("fx", "Firefox"), None);
    }
}
