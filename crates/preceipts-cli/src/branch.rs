//! Turning a genesis prompt into a branch name.
//!
//! Local and instant by design. The doc is explicit that naming must never
//! make someone wait for a worktree, and that any model call is optional,
//! non-blocking, and vendor-neutral — so the default path is a slug heuristic
//! with no network and no key.

use preceipts_core::workspace::slug;

/// Words that carry no signal in a branch name. Deliberately short: this is a
/// stop list, not an attempt at language understanding, and over-trimming
/// produces worse names than under-trimming.
const NOISE: &[&str] = &[
    "a", "an", "the", "to", "for", "of", "in", "on", "at", "by", "with", "and", "or", "please",
    "can", "you", "we", "i", "it", "this", "that", "make", "let", "lets", "so", "then", "when",
    "if", "is", "are", "be", "should", "would", "just", "some", "my", "our",
];

/// How many words survive. Long enough to be specific, short enough to read
/// in a tab.
const MAX_WORDS: usize = 5;

/// A branch name from stated intent.
///
/// "fix the checkout race so payments settle" → `fix-checkout-race-payments`
pub fn from_prompt(prompt: &str) -> String {
    let words: Vec<String> = prompt
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| !NOISE.contains(&w.as_str()))
        .take(MAX_WORDS)
        .collect();

    if words.is_empty() {
        // Every word was noise, or the prompt was punctuation. Fall back to
        // slugging the raw text rather than inventing a name.
        return slug(prompt);
    }
    slug(&words.join("-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_meaningful_words() {
        assert_eq!(
            from_prompt("fix the checkout race so payments settle"),
            "fix-checkout-race-payments-settle"
        );
    }

    #[test]
    fn caps_the_length() {
        let name = from_prompt("one two three four five six seven eight nine");
        assert_eq!(name.split('-').count(), MAX_WORDS);
    }

    #[test]
    fn survives_punctuation_and_case() {
        assert_eq!(
            from_prompt("Fix: the `Checkout` bug!!!"),
            "fix-checkout-bug"
        );
    }

    #[test]
    fn all_noise_falls_back_rather_than_inventing() {
        assert_eq!(from_prompt("the a of"), "the-a-of");
    }

    #[test]
    fn empty_intent_still_produces_a_usable_name() {
        assert_eq!(from_prompt(""), "workspace");
        assert_eq!(from_prompt("!!!"), "workspace");
    }

    #[test]
    fn names_are_dns_safe() {
        let name = from_prompt("Add JIRA-123 support/for émigré users");
        assert!(name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert!(!name.starts_with('-') && !name.ends_with('-'));
    }
}
