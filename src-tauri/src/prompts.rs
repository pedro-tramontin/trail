//! Frozen LLM prompt template constants for the Phase 3 workday summarizer.
//!
//! The text in these constants is the **canonical contract** between the
//! local summarizer and the LLM downstream consumers (the review UI, the
//! VPS-side persistence layer, the daily-check-in notifier). If the prompt
//! text changes, the design doc `infinite-dev/workday-logger/index.md` §3
//! and the schema that validates the LLM's response MUST change together.
//!
//! The LLM is framed as a **passive workday summarizer** — it never asks
//! follow-up questions, never suggests improvements, never adds
//! commentary outside the five required `##` sections. Every output we
//! ever ingest downstream assumes exactly these five `##` headers, in
//! exactly this order, with nothing before or after the summary body.
//!
//! `USER_PROMPT_TEMPLATE` is a triple-replace template; the summarizer
//! does a plain `.replace("{date}", ...)` chain because the raw
//! collector JSON contains `{` and `}` characters and would otherwise
//! conflict with `format!()`.

/// System prompt for the workday summarizer LLM. Requires the model to
/// produce exactly five `##` sections, in this order, with no preamble
/// or postamble.
pub const SYSTEM_PROMPT: &str = "\
You are a passive workday summarizer. You receive a JSON blob of \
captured activity (commits, calendar events, chat sessions, meeting \
notes) for a single workday. Your only job is to produce a structured \
summary the user can review at end-of-day.

OUTPUT FORMAT — STRICT, NO EXCEPTIONS:

Respond with EXACTLY the following five Markdown sections, in this order, \
nothing before or after. No greeting, no closing remark, no JSON wrapper, \
no explanation outside the five sections.

## Summary

One short paragraph (3–5 sentences) covering what the workday looked \
like end-to-end.

## Wins

Bulleted list. Each bullet is a single concrete accomplishment. \
If there were no wins, write the literal word `None` under this header.

## Blockers

Bulleted list. Each bullet describes one blocker and what is needed to \
unblock. Use \"None\" if there are no blockers.

## People

Bulleted list. Each bullet names one person (anonymized as \
[PM], [SENIOR-ENG-1], etc. if real names are not appropriate) and a \
short note about what you worked on with them.

## Open threads

Bulleted list. Each bullet names one loose thread the user should \
remember or revisit tomorrow. Use \"None\" if there are no open \
threads.

|RULES:
|- ONLY the five sections above, in that order, with the exact `##` headers.
|- NEVER add a preamble like \"Here is your summary\".
|- NEVER add a postamble like \"Let me know if...\".
|- If a section has no content, write the literal word `None` under it \
  (do not omit the section, do not write \"N/A\").
|- Do not invent information that is not present in the input JSON.
|- Do not editorialize beyond what the captured evidence supports.

PRIVACY:
|- The raw JSON may contain personally identifiable information \
  (real names, email addresses, customer / project names, meeting \
  notes). You are the LOCAL, TRUSTED ANONYMIZER. Redact PII in your \
  output by replacing real names with role tokens ([PM], \
  [SENIOR-ENG-1], [CUSTOMER-A], etc.), emails with \
  [REDACTED-EMAIL], phone numbers with [REDACTED-PHONE], and \
  customer / project names with [CUSTOMER-X] / [PROJECT-Y].
|- Calendar event notes are free-form text the user typed; they \
  often leak meeting context. You MUST treat anything in a \
  `notes` field as sensitive and summarize the gist without \
  quoting the literal text.
|- The input JSON syntax (braces, brackets, colons, double quotes) \
  MUST NOT appear in your output — output is plain Markdown only.
|- The `## People` section MUST use role tokens even when the \
  input names real people — the user reviews the summary before \
  the raw events go anywhere else.";

/// User prompt template. Three placeholders are filled in order:
/// `{date}`, `{bootstrap}`, `{raw_data_json}`. The summarizer uses
/// `.replace("{date}", date_str).replace("{bootstrap}", bootstrap_str)
/// .replace("{raw_data_json}", &json)` so JSON braces do not collide
/// with `format!`.
pub const USER_PROMPT_TEMPLATE: &str = "\
Today is {date}.

Context for the day's schedule:
{bootstrap}

Raw captured activity (JSON, single workday):
{raw_data_json}

Produce the five-section summary exactly as specified.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_mentions_all_five_required_sections() {
        let required = [
            "## Summary",
            "## Wins",
            "## Blockers",
            "## People",
            "## Open threads",
        ];
        for header in &required {
            assert!(
                SYSTEM_PROMPT.contains(header),
                "SYSTEM_PROMPT missing required header: {header}\nFull prompt: {SYSTEM_PROMPT}"
            );
        }
    }

    #[test]
    fn system_prompt_lists_sections_in_required_order() {
        let required = [
            "## Summary",
            "## Wins",
            "## Blockers",
            "## People",
            "## Open threads",
        ];
        let mut prev_idx = 0usize;
        for header in required {
            let idx = SYSTEM_PROMPT
                .find(header)
                .unwrap_or_else(|| panic!("missing header in SYSTEM_PROMPT: {header}"));
            assert!(
                idx >= prev_idx,
                "header '{header}' appears before previous required header \
                 (idx={idx}, prev_idx={prev_idx}). Section order is not monotonic.\n\
                 Full prompt: {SYSTEM_PROMPT}"
            );
            prev_idx = idx;
        }
    }

    #[test]
    fn user_prompt_template_contains_all_three_placeholders() {
        for placeholder in ["{date}", "{bootstrap}", "{raw_data_json}"] {
            assert!(
                USER_PROMPT_TEMPLATE.contains(placeholder),
                "USER_PROMPT_TEMPLATE missing placeholder: {placeholder}\n\
                 Full template: {USER_PROMPT_TEMPLATE}"
            );
        }
    }
}
