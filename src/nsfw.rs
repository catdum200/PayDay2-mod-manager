//! NSFW detection.
//!
//! Nexus Mods flags adult content explicitly. ModWorkshop has no NSFW flag, so
//! mods there are checked against a keyword list (name, summary, description, tags).
//! The check is deliberately cautious: when in doubt, a mod is treated as NSFW.

const KEYWORDS: &[&str] = &[
    "nsfw", "18+", "r18", "r-18", "nude", "nudes", "nudity", "naked", "lewd", "hentai", "porn",
    "porno", "pornographic", "ecchi", "erotic", "erotica", "xxx", "topless", "nipple", "nipples",
    "boobs", "boob", "tits", "titty", "titties", "breasts", "oppai", "lingerie", "thicc",
    "futa", "futanari", "rule34", "r34", "uncensored", "sexy", "sex", "sexual", "stripper",
    "bikini", "panties", "underwear", "ahegao", "waifu body", "onlyfans", "adult only",
    "adults only", "explicit",
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '+' || c == '-'
}

/// True if `text` contains any keyword as a whole word/phrase.
pub fn text_is_nsfw(text: &str) -> bool {
    let lower = text.to_lowercase();
    for kw in KEYWORDS {
        let mut start = 0;
        while let Some(pos) = lower[start..].find(kw) {
            let i = start + pos;
            let before = lower[..i].chars().next_back();
            let after = lower[i + kw.len()..].chars().next();
            let ok_before = before.map(|c| !is_word_char(c)).unwrap_or(true);
            let ok_after = after.map(|c| !is_word_char(c)).unwrap_or(true);
            if ok_before && ok_after {
                return true;
            }
            start = i + kw.len();
        }
    }
    false
}

pub fn mod_is_nsfw(name: &str, summary: &str, description: &str, tags: &[String]) -> bool {
    if text_is_nsfw(name) || text_is_nsfw(summary) {
        return true;
    }
    if tags.iter().any(|t| text_is_nsfw(t)) {
        return true;
    }
    // Only scan the start of long descriptions to limit false positives from changelogs.
    let desc: String = description.chars().take(1500).collect();
    text_is_nsfw(&desc)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects() {
        assert!(text_is_nsfw("Nude skin pack"));
        assert!(text_is_nsfw("(NSFW) thing"));
        assert!(text_is_nsfw("18+ only"));
        assert!(!text_is_nsfw("Sussex heist"));
        assert!(!text_is_nsfw("Better HUD"));
        assert!(!text_is_nsfw("Essex map"));
    }
}
