/// Score a case-insensitive command query. Higher is better.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let candidate = candidate.to_lowercase();
    if candidate == query {
        return Some(1000);
    }
    if candidate.starts_with(&query) {
        return Some(800 - candidate.len() as i32);
    }
    if let Some(position) = candidate.find(&query) {
        return Some(600 - position as i32);
    }

    let mut score = 0;
    let mut query_chars = query.chars();
    let mut wanted = query_chars.next()?;
    for (index, ch) in candidate.chars().enumerate() {
        if ch == wanted {
            score += 20 - (index as i32 / 4);
            wanted = match query_chars.next() {
                Some(next) => next,
                None => return Some(score),
            };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_search_ranks_prefix_then_word_then_subsequence() {
        let exact = fuzzy_score("goal", "goal").unwrap();
        let prefix = fuzzy_score("go", "goal").unwrap();
        let word = fuzzy_score("al", "goal").unwrap();
        let subsequence = fuzzy_score("tsd", "test-driven-development").unwrap();
        assert!(exact > prefix);
        assert!(prefix > word);
        assert!(subsequence > 0);
    }
}
