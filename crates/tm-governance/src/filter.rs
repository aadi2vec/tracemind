use tm_types::{Result, TraceMindError};

pub struct GovernanceFilter {
    confidence_threshold: f64,
}

impl GovernanceFilter {
    pub fn new(confidence_threshold: f64) -> Self {
        GovernanceFilter { confidence_threshold }
    }

    pub fn check(&self, text: &str, confidence: f64) -> Result<()> {
        if Self::contains_pii(text) {
            return Err(TraceMindError::PiiDetected);
        }
        if confidence < self.confidence_threshold {
            return Err(TraceMindError::ConfidenceBelowThreshold {
                confidence,
                threshold: self.confidence_threshold,
            });
        }
        Ok(())
    }

    fn contains_pii(text: &str) -> bool {
        Self::has_email(text)
            || Self::has_phone(text)
            || Self::has_ssn(text)
            || Self::has_credit_card(text)
    }

    /// Email: contains '@', at least one char before it, and a '.' with >=2 chars after it in the
    /// domain part.
    fn has_email(text: &str) -> bool {
        for (i, _) in text.match_indices('@') {
            // Must have at least one non-space, non-@ char before '@'
            let before = &text[..i];
            let local_start = before
                .rfind(|c: char| !c.is_alphanumeric() && c != '.' && c != '_' && c != '%' && c != '+' && c != '-')
                .map(|p| p + 1)
                .unwrap_or(0);
            let local = &before[local_start..];
            if local.is_empty() {
                continue;
            }

            // Domain part after '@'
            let after = &text[i + 1..];
            // Find the end of the domain token (stop at whitespace or non-domain chars)
            let domain_end = after
                .find(|c: char| c.is_whitespace() || (!c.is_alphanumeric() && c != '.' && c != '-'))
                .unwrap_or(after.len());
            let domain = &after[..domain_end];

            // Domain must contain a '.' and have at least 2 chars after the last '.'
            if let Some(dot_pos) = domain.rfind('.') {
                let tld = &domain[dot_pos + 1..];
                if tld.len() >= 2 && !domain[..dot_pos].is_empty() {
                    return true;
                }
            }
        }
        false
    }

    /// Phone: a sequence of 10 consecutive digits (ignoring spaces, dashes, parens) or +1 followed
    /// by 10 digits (ignoring the same separators).
    /// Whether the character at `idx` is a letter, i.e. the digit run is
    /// glued to an identifier rather than standing alone.
    ///
    /// Without this guard, hex strings match the numeric detectors: a UUID
    /// or commit SHA contains long digit runs separated by dashes, and
    /// "collect ten digits skipping dashes" happily consumes them. That
    /// silently rejects exactly the content TraceMind exists to capture —
    /// UUIDs, SHAs, hashes and IDs are everywhere in a developer's
    /// clipboard — and the rejection is invisible, because the ingest gate
    /// simply drops the memory.
    fn is_letter_at(chars: &[char], idx: usize) -> bool {
        chars.get(idx).map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
    }

    /// A numeric run is only PII-like if no whitespace-delimited token it
    /// spans contains a letter.
    ///
    /// Checking only the immediately adjacent characters is not enough: a
    /// ten-digit run inside a UUID often ends on a `-`, so the next
    /// character is a separator rather than the hex letter that follows it.
    /// Widening to the enclosing token is what actually distinguishes
    /// `550e8400-e29b-41d4-…` from `415-555-0132`.
    fn has_token_boundaries(chars: &[char], start: usize, end: usize) -> bool {
        let n = chars.len();
        // Expand to the whitespace-delimited span containing [start, end).
        let mut lo = start.min(n);
        while lo > 0 && !chars[lo - 1].is_whitespace() {
            lo -= 1;
        }
        let mut hi = end.min(n);
        while hi < n && !chars[hi].is_whitespace() {
            hi += 1;
        }
        !chars[lo..hi].iter().any(|c| c.is_ascii_alphabetic())
    }

    /// Total digits in the whitespace-delimited token spanning `[start, end)`.
    ///
    /// Length is the other half of the identifier test. A phone number's
    /// token holds about ten digits; a UUID's holds thirty-two. Matching
    /// "ten digits, ignoring dashes" therefore fires on the *first ten* of a
    /// far longer identifier, and checking only the adjacent character
    /// cannot see that — the eleventh digit is usually behind a dash.
    fn token_digit_count(chars: &[char], start: usize, end: usize) -> usize {
        let n = chars.len();
        let mut lo = start.min(n);
        while lo > 0 && !chars[lo - 1].is_whitespace() {
            lo -= 1;
        }
        let mut hi = end.min(n);
        while hi < n && !chars[hi].is_whitespace() {
            hi += 1;
        }
        chars[lo..hi].iter().filter(|c| c.is_ascii_digit()).count()
    }

    fn has_phone(text: &str) -> bool {
        // Strip separators and look for 10-digit (or +1 + 10-digit) runs in the original text by
        // scanning windows of characters.
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut i = 0;
        while i < n {
            // Check for optional +1 prefix
            let mut j = i;
            let mut has_plus_one = false;
            if j < n && chars[j] == '+' {
                j += 1;
                if j < n && chars[j] == '1' {
                    j += 1;
                    // skip separator after +1
                    while j < n && (chars[j] == ' ' || chars[j] == '-') {
                        j += 1;
                    }
                    has_plus_one = true;
                } else {
                    // not +1, reset
                    i += 1;
                    continue;
                }
            }

            // Now try to collect 10 digits (skipping separators)
            let start = j;
            let mut digits = 0usize;
            let mut k = start;
            while k < n && digits < 10 {
                let c = chars[k];
                if c.is_ascii_digit() {
                    digits += 1;
                    k += 1;
                } else if c == ' ' || c == '-' || c == '(' || c == ')' {
                    k += 1;
                } else {
                    break;
                }
            }

            if digits == 10 {
                // Make sure there are no extra digits immediately after (avoid matching part of
                // a longer digit sequence that isn't a phone)
                let trailing_digit = k < n && chars[k].is_ascii_digit();
                // …and that the run is not glued to letters, which is what
                // a hex identifier looks like.
                // A phone token carries ~10-11 digits; anything materially
                // longer is an identifier that merely starts with ten.
                let plausible_length = Self::token_digit_count(&chars, i, k) <= 11;
                if !trailing_digit
                    && plausible_length
                    && Self::has_token_boundaries(&chars, i, k)
                {
                    // Also check that the run we consumed was actually phone-like: if no +1 prefix,
                    // ensure this 10-digit run isn't just a raw unbroken 16-digit CC number.
                    // We'll let has_credit_card handle that; accept here.
                    return true;
                }
            }

            if has_plus_one {
                // Already advanced past +1, skip to avoid re-scanning same '+'
                i = start;
            } else {
                i += 1;
            }
        }
        false
    }

    /// SSN: substring matching DDD-DD-DDDD exactly (3 digits, dash, 2 digits, dash, 4 digits).
    fn has_ssn(text: &str) -> bool {
        let bytes = text.as_bytes();
        let len = bytes.len();
        // Minimum length: 11 chars (3+1+2+1+4)
        if len < 11 {
            return false;
        }
        for i in 0..=(len - 11) {
            // Check 3 digits
            if !bytes[i].is_ascii_digit()
                || !bytes[i + 1].is_ascii_digit()
                || !bytes[i + 2].is_ascii_digit()
            {
                continue;
            }
            if bytes[i + 3] != b'-' {
                continue;
            }
            // Check 2 digits
            if !bytes[i + 4].is_ascii_digit() || !bytes[i + 5].is_ascii_digit() {
                continue;
            }
            if bytes[i + 6] != b'-' {
                continue;
            }
            // Check 4 digits
            if !bytes[i + 7].is_ascii_digit()
                || !bytes[i + 8].is_ascii_digit()
                || !bytes[i + 9].is_ascii_digit()
                || !bytes[i + 10].is_ascii_digit()
            {
                continue;
            }
            // Ensure no extra digits immediately adjacent (word boundary)
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
            let after_ok = (i + 11) >= len || !bytes[i + 11].is_ascii_digit();
            let chars: Vec<char> = text.chars().collect();
            if before_ok
                && after_ok
                && Self::token_digit_count(&chars, i, i + 11) <= 9
                && Self::has_token_boundaries(&chars, i, i + 11)
            {
                return true;
            }
        }
        false
    }

    /// Credit card: 16 consecutive digits ignoring spaces and dashes.
    fn has_credit_card(text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let mut i = 0;
        while i < n {
            if !chars[i].is_ascii_digit() {
                i += 1;
                continue;
            }
            // Try to collect 16 digits from position i, skipping spaces/dashes
            let mut digits = 0usize;
            let mut k = i;
            while k < n && digits < 16 {
                let c = chars[k];
                if c.is_ascii_digit() {
                    digits += 1;
                    k += 1;
                } else if c == ' ' || c == '-' {
                    k += 1;
                } else {
                    break;
                }
            }
            if digits == 16 {
                // No extra digit on either side, and not embedded in an
                // identifier (hex strings are digits plus a-f).
                let before_ok = i == 0 || !chars[i - 1].is_ascii_digit();
                let after_ok = k >= n || !chars[k].is_ascii_digit();
                if before_ok
                    && after_ok
                    && Self::token_digit_count(&chars, i, k) <= 16
                    && Self::has_token_boundaries(&chars, i, k)
                {
                    return true;
                }
            }
            i += 1;
        }
        false
    }
}

impl Default for GovernanceFilter {
    fn default() -> Self {
        GovernanceFilter::new(0.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tm_types::TraceMindError;

    fn filter() -> GovernanceFilter {
        GovernanceFilter::default()
    }

    #[test]
    fn email_detected() {
        let result = filter().check("contact me at foo@bar.com", 1.0);
        assert!(matches!(result, Err(TraceMindError::PiiDetected)));
    }

    #[test]
    fn ssn_detected() {
        let result = filter().check("ssn is 123-45-6789", 1.0);
        assert!(matches!(result, Err(TraceMindError::PiiDetected)));
    }

    #[test]
    fn phone_detected() {
        let result = filter().check("call 555-867-5309", 1.0);
        assert!(matches!(result, Err(TraceMindError::PiiDetected)));
    }

    #[test]
    fn clean_text_high_confidence() {
        let result = filter().check("the weather is nice today", 0.9);
        assert!(result.is_ok());
    }

    #[test]
    fn clean_text_low_confidence() {
        let result = filter().check("the weather is nice today", 0.1);
        assert!(matches!(
            result,
            Err(TraceMindError::ConfidenceBelowThreshold { confidence, threshold })
            if (confidence - 0.1).abs() < 1e-9 && (threshold - 0.4).abs() < 1e-9
        ));
    }

    #[test]
    fn clean_text_at_threshold() {
        let result = filter().check("the weather is nice today", 0.4);
        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod identifier_false_positive_tests {
    use super::*;

    /// UUIDs, commit SHAs and hashes are ubiquitous in the content
    /// TraceMind captures. Treating them as PII silently drops the memory,
    /// and the drop is invisible to the user.
    #[test]
    fn uuids_are_not_pii() {
        // Digit-heavy UUIDs that previously tripped the phone detector.
        for uuid in [
            "550e8400-e29b-41d4-a716-446655440000",
            "12345678-9012-3456-7890-123456789012",
            "00112233-4455-6677-8899-001122334455",
        ] {
            let text = format!("Deployed build {uuid} to staging");
            assert!(
                !GovernanceFilter::contains_pii(&text),
                "UUID flagged as PII: {uuid}"
            );
        }
    }

    #[test]
    fn commit_shas_are_not_pii() {
        let text = "Reverted a1b2c3d4556677889900112233445566778899 after the outage";
        assert!(!GovernanceFilter::contains_pii(text), "SHA flagged as PII");
    }

    /// The detectors must still catch the real thing.
    #[test]
    fn real_phone_numbers_are_still_detected() {
        assert!(GovernanceFilter::contains_pii("call me at 415-555-0132"));
        assert!(GovernanceFilter::contains_pii("+1 415 555 0132"));
    }

    #[test]
    fn real_ssn_is_still_detected() {
        assert!(GovernanceFilter::contains_pii("ssn 123-45-6789"));
    }

    #[test]
    fn real_credit_card_is_still_detected() {
        assert!(GovernanceFilter::contains_pii("card 4111 1111 1111 1111"));
    }
}
