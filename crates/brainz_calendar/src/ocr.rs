//! Brainz: text out of a pasted screenshot, through the bundled
//! `brainz-ocr` helper (Vision framework), plus the email heuristics and the
//! pre-filled messages behind the composer's Log correspondence, Draft
//! reply, and File in folder chips.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;

use crate::{
    brain_config::BrainConfig,
    brain_match::{Match, emails_in},
};

const HOUSE_RULES: &str =
    "House rules: no em dashes, absolute dates, headings for structure only.";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmailFacts {
    pub is_email: bool,
    pub sender_name: Option<String>,
    pub sender_email: Option<String>,
    pub subject: Option<String>,
}

impl EmailFacts {
    pub fn sender_domain(&self) -> Option<&str> {
        self.sender_email
            .as_deref()
            .and_then(|email| email.rsplit_once('@'))
            .map(|(_, domain)| domain)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct HelperOutput {
    status: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    error: Option<String>,
}

fn helper_path() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating Brainz executable")?;
    let dir = exe.parent().context("Brainz executable has no parent")?;
    let helper = dir.join("brainz-ocr");
    if helper.is_file() {
        Ok(helper)
    } else {
        Err(anyhow!(
            "OCR helper missing at {}; run script/brainz-local",
            helper.display()
        ))
    }
}

/// Blocking on purpose: only ever called from the background executor.
#[allow(clippy::disallowed_methods)]
pub fn recognize_text(image: &Path) -> Result<String> {
    let helper = helper_path()?;
    let output = std::process::Command::new(&helper)
        .arg(image)
        .output()
        .with_context(|| format!("running {}", helper.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "OCR helper exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let parsed: HelperOutput =
        serde_json::from_slice(&output.stdout).context("parsing OCR helper output")?;
    match parsed.status.as_str() {
        "ok" => Ok(parsed.text),
        other => Err(anyhow!(
            "OCR {other}: {}",
            parsed.error.unwrap_or_default()
        )),
    }
}

fn header_value<'a>(line: &'a str, header: &str) -> Option<&'a str> {
    let trimmed = line.trim();
    if trimmed.len() < header.len() + 1 {
        return None;
    }
    let (head, rest) = trimmed.split_at(header.len());
    (head.eq_ignore_ascii_case(header) && rest.starts_with(':')).then(|| rest[1..].trim())
}

fn looks_like_person(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    (2..=3).contains(&words.len())
        && words.iter().all(|word| {
            word.chars().next().is_some_and(|c| c.is_uppercase())
                && word
                    .chars()
                    .all(|c| c.is_alphabetic() || c == '\'' || c == '-' || c == '.')
        })
}

/// Splits `Grace Hopper <grace@acme.com>` into its parts.
fn name_and_email(text: &str) -> (Option<String>, Option<String>) {
    let email = emails_in(text).into_iter().next();
    let name = text
        .split(['<', '('])
        .next()
        .map(|name| name.trim().trim_matches('"').trim_end_matches(',').trim().to_owned())
        .filter(|name| looks_like_person(name));
    (name, email)
}

/// Heuristics over OCR text for the shapes mail clients produce: `From:`
/// headers (Apple Mail, Outlook), `Name <email>` lines (Gmail), and a
/// sender name sitting on its own line above the addressing.
pub fn parse_email(text: &str) -> EmailFacts {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let emails = emails_in(text);
    let mut facts = EmailFacts::default();
    let lower = text.to_lowercase();
    let email_markers = ["wrote:", "sent from my", "unsubscribe", "forwarded message"];
    let has_header = lines.iter().any(|line| {
        ["from", "to", "cc", "subject", "re", "fwd", "fw", "date"]
            .iter()
            .any(|header| header_value(line, header).is_some())
    });
    let to_me = lines
        .iter()
        .any(|line| line.eq_ignore_ascii_case("to me") || line.to_lowercase().starts_with("to me "));
    facts.is_email = !emails.is_empty()
        || has_header
        || to_me
        || email_markers.iter().any(|marker| lower.contains(marker));
    if !facts.is_email {
        return facts;
    }

    for line in &lines {
        if let Some(value) = header_value(line, "subject") {
            facts.subject = Some(value.to_owned());
            break;
        }
    }

    if let Some(from) = lines
        .iter()
        .find_map(|line| header_value(line, "from"))
    {
        let (name, email) = name_and_email(from);
        facts.sender_name = name;
        facts.sender_email = email;
        if facts.sender_name.is_none() && facts.sender_email.is_none() && looks_like_person(from) {
            facts.sender_name = Some(from.to_owned());
        }
    }

    if facts.sender_email.is_none() {
        // The first address that is not inside a To/Cc line is the sender.
        for line in &lines {
            let is_recipient_line = ["to", "cc", "bcc"]
                .iter()
                .any(|header| header_value(line, header).is_some())
                || line.to_lowercase().starts_with("to ");
            if is_recipient_line {
                continue;
            }
            let (name, email) = name_and_email(line);
            if let Some(email) = email {
                facts.sender_email = Some(email);
                if facts.sender_name.is_none() {
                    facts.sender_name = name;
                }
                break;
            }
        }
    }

    if facts.sender_name.is_none() {
        // Gmail shows the sender's name alone on the line above "to me".
        let stop = lines
            .iter()
            .position(|line| {
                line.eq_ignore_ascii_case("to me")
                    || line.to_lowercase().starts_with("to me ")
                    || line.to_lowercase().starts_with("to ")
                    || emails_in(line).first().is_some_and(|e| Some(e) == facts.sender_email.as_ref())
            })
            .unwrap_or(lines.len());
        facts.sender_name = lines[..stop]
            .iter()
            .rev()
            .find(|line| looks_like_person(line))
            .map(|line| (*line).to_owned());
    }
    facts
}

/// The messages the chips pre-fill. `owner` is the brain owner's name for
/// the Draft reply voice ("Tommy's reply"), or "my" when unknown.
pub struct Prompts<'a> {
    pub config: &'a BrainConfig,
}

impl Prompts<'_> {
    fn owner_possessive(&self) -> String {
        if self.config.stop_words.is_empty() {
            "my".to_owned()
        } else {
            format!("{}'s", self.config.owner_label())
        }
    }

    pub fn log_correspondence(&self, found: Option<&Match>, ocr_text: &str) -> String {
        let mut text = match found {
            Some(found) => format!(
                "Log this email in `{}`'s correspondence log, verbatim with a read, and update the folder's status callout. {HOUSE_RULES}",
                found.folder
            ),
            None => format!(
                "Log this email in the right folder's correspondence log, verbatim with a read, and update that folder's status callout. Find the folder that matches the sender and say which one you chose. {HOUSE_RULES}"
            ),
        };
        append_ocr(&mut text, ocr_text);
        text
    }

    pub fn draft_reply(&self, found: Option<&Match>, ocr_text: &str) -> String {
        let folder = found
            .map(|found| format!("`{}`'s correspondence log", found.folder))
            .unwrap_or_else(|| "the matching folder's correspondence log (find it from the sender and say which)".to_owned());
        let owner = self.owner_possessive();
        let mut text = format!(
            "Draft {owner} reply to this. First open {folder} and read the last two outbound messages in this thread for voice and prior positions. Pull numbers only from the brain, never approximate. No em dashes. Give the draft and one or two notes on what it does and doesn't say."
        );
        append_ocr(&mut text, ocr_text);
        text
    }

    pub fn file_in_folder(&self, folder: &str, files: &[String]) -> String {
        let references = files
            .iter()
            .map(|file| format!("`{folder}/{file}`"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Describe this in one line in `{folder}/CLAUDE.md` next to the file reference {references}. {HOUSE_RULES}"
        )
    }
}

fn append_ocr(text: &mut String, ocr_text: &str) {
    let ocr_text = ocr_text.trim();
    if ocr_text.is_empty() {
        text.push_str("\n\n(No text could be read from the screenshot; read the image.)");
    } else {
        text.push_str("\n\nText read from the screenshot:\n```\n");
        text.push_str(&ocr_text.replace("```", "'''"));
        text.push_str("\n```");
    }
}

/// `2026-10-01-screenshot-1.png`, the media rule's date-prefixed name.
pub fn dated_file_name(date: chrono::NaiveDate, ordinal: usize, extension: &str) -> String {
    format!(
        "{}-screenshot-{ordinal}.{extension}",
        date.format("%Y-%m-%d")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmail_shape_name_above_to_me() {
        let facts = parse_email(
            "Grace Hopper\nto me\nHi Ada,\nThanks for confirming. Alan is set for Thu 10/1.\nGrace",
        );
        assert!(facts.is_email);
        assert_eq!(facts.sender_name.as_deref(), Some("Grace Hopper"));
        assert_eq!(facts.sender_email, None);
    }

    #[test]
    fn gmail_expanded_header_with_address() {
        let facts = parse_email(
            "Grace Hopper <grace@acme.com>\nto Ada\nWed, Sep 30, 11:39 AM\nTeam debrief Thu 10/1.",
        );
        assert_eq!(facts.sender_name.as_deref(), Some("Grace Hopper"));
        assert_eq!(facts.sender_email.as_deref(), Some("grace@acme.com"));
        assert_eq!(facts.sender_domain(), Some("acme.com"));
    }

    #[test]
    fn apple_mail_headers() {
        let facts = parse_email(
            "From: Alan Turing <alan@acme.com>\nSubject: Re: Great meeting you\nDate: September 25, 2026 at 6:32 PM\nTo: Ada Lovelace <ada@example.com>\n\nIt was great meeting you, man!",
        );
        assert!(facts.is_email);
        assert_eq!(facts.sender_name.as_deref(), Some("Alan Turing"));
        assert_eq!(facts.sender_email.as_deref(), Some("alan@acme.com"));
        assert_eq!(facts.subject.as_deref(), Some("Re: Great meeting you"));
    }

    #[test]
    fn recipient_lines_do_not_become_the_sender() {
        let facts = parse_email(
            "To: ada@example.com\nCc: grace@acme.com\nHank Scorpio <hank@acme.com>\nwe've just reopened our role",
        );
        assert_eq!(facts.sender_email.as_deref(), Some("hank@acme.com"));
        assert_eq!(facts.sender_name.as_deref(), Some("Hank Scorpio"));
    }

    #[test]
    fn a_dashboard_screenshot_is_not_an_email() {
        let facts = parse_email("Weekly Active Users\n12,480\n+4.2% vs last week\nRetention 38%");
        assert!(!facts.is_email);
        assert_eq!(facts.sender_name, None);
    }

    #[test]
    fn prompts_name_the_folder_and_carry_the_text() {
        let config = BrainConfig {
            stop_words: vec!["tommy".into()],
            ..BrainConfig::default()
        };
        let prompts = Prompts { config: &config };
        let found = Match {
            folder: "companies/acme".into(),
            folder_name: "Acme".into(),
            who: "Grace Hopper".into(),
            prep_file: None,
            score: 8,
        };
        let text = prompts.log_correspondence(Some(&found), "Grace Hopper\nto me\nHi");
        assert!(text.starts_with("Log this email in `companies/acme`'s correspondence log"));
        assert!(text.contains("```\nGrace Hopper\nto me\nHi\n```"));
        assert!(!text.contains('—'));
        let text = prompts.log_correspondence(None, "");
        assert!(text.contains("say which one you chose"));
        assert!(text.contains("No text could be read"));
        let text = prompts.draft_reply(Some(&found), "x");
        assert!(text.starts_with("Draft Ada's reply to this. First open `companies/acme`'s correspondence log"));
        let generic = BrainConfig::default();
        let prompts = Prompts { config: &generic };
        assert!(prompts.draft_reply(None, "x").starts_with("Draft my reply"));
        assert_eq!(
            prompts.file_in_folder("companies/acme", &["2026-10-01-screenshot-1.png".into()]),
            format!("Describe this in one line in `companies/acme/CLAUDE.md` next to the file reference `companies/acme/2026-10-01-screenshot-1.png`. {HOUSE_RULES}")
        );
        assert_eq!(
            dated_file_name(chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), 2, "png"),
            "2026-10-01-screenshot-2.png"
        );
    }
}
