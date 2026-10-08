//! Translators: backends that turn a batch of entries into translations.

use std::collections::HashMap;
use std::time::Duration;

use kintsugi_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::entry::TranslationEntry;

/// Translates a batch of entries from one language into another.
pub trait Translator: Send + Sync {
    /// Backend name for reports.
    fn name(&self) -> &str;

    /// Translate `entries`; the result must align with the input by `id`.
    ///
    /// Backends may leave a line untouched (unknown word, command-like
    /// text) — an entry always comes back for every entry in.
    fn translate(
        &self,
        entries: &[TranslationEntry],
        source_lang: &str,
        target_lang: &str,
    ) -> Result<Vec<TranslationEntry>>;
}

/// Term pairs kept consistent across every batch.
///
/// Character names, honorifics, in-world jargon — the things a model
/// reinvents differently on every call unless pinned.
#[derive(Clone, Debug, Default)]
pub struct Glossary {
    /// `(source term, target term)` pairs.
    pub terms: Vec<(String, String)>,
}

impl Glossary {
    /// An empty glossary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pin one term.
    pub fn term(mut self, source: impl Into<String>, target: impl Into<String>) -> Self {
        self.terms.push((source.into(), target.into()));
        self
    }

    /// Whether any terms are pinned.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// The glossary section of the system prompt.
    fn prompt_fragment(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = String::from("\nGlossary (always use these renderings):\n");
        for (source, target) in &self.terms {
            out.push_str(&format!("- {source} → {target}\n"));
        }
        out
    }
}

/// What happened in one batch — for hosts that like visible numbers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BatchOutput {
    /// Zero-based batch index.
    pub batch: usize,
    /// Entries sent in this batch.
    pub requested: usize,
    /// Entries that came back translated.
    pub translated: usize,
    /// Entries returned unchanged (command-like, or missed by the model).
    pub unchanged: usize,
    /// Ids in the reply that matched no request (hallucinations, dropped).
    pub extra_ids: usize,
}

/// A [`Translator`] backed by any OpenAI-compatible chat API.
///
/// Works with OpenAI, DeepSeek, vLLM, Ollama (`/v1` endpoint), LM Studio,
/// and anything else that speaks `/chat/completions`. The reply must be a
/// JSON object: `{"translations":[{"id":…,"speaker":…,"text":…}]}` — the
/// prompt demands it, and the parser enforces it.
pub struct LlmTranslator {
    agent: ureq::Agent,
    /// e.g. `https://api.openai.com/v1` — no trailing slash needed.
    pub api_base: String,
    /// Bearer token; empty string for keyless local servers.
    pub api_key: String,
    /// Model name, e.g. `gpt-4o-mini`, `deepseek-chat`, `qwen2.5:14b`.
    pub model: String,
    /// Terms pinned across all batches.
    pub glossary: Glossary,
    /// Entries per request (default 40).
    pub batch_size: usize,
    /// Attempts per batch before giving up (default 3).
    pub max_attempts: usize,
    /// Sampling temperature (default 0.3 — translation wants low heat).
    pub temperature: f32,
}

impl LlmTranslator {
    /// A translator for `api_base` / `model`, defaults elsewhere.
    pub fn new(
        api_base: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout_read(Duration::from_secs(180))
                .timeout_write(Duration::from_secs(30))
                .build(),
            api_base: api_base.into(),
            api_key: api_key.into(),
            model: model.into(),
            glossary: Glossary::new(),
            batch_size: 40,
            max_attempts: 3,
            temperature: 0.3,
        }
    }

    /// Pin glossary terms.
    pub fn with_glossary(mut self, glossary: Glossary) -> Self {
        self.glossary = glossary;
        self
    }

    /// Override the batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// Translate and also report per-batch statistics.
    pub fn translate_with_report(
        &self,
        entries: &[TranslationEntry],
        source_lang: &str,
        target_lang: &str,
    ) -> Result<(Vec<TranslationEntry>, Vec<BatchOutput>)> {
        if entries.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut out = Vec::with_capacity(entries.len());
        let mut reports = Vec::new();

        // Command-like lines never reach the model: pure-ASCII text in a
        // non-Latin source game is an engine opcode wearing a text costume.
        let latin_source = matches!(
            source_lang.to_ascii_lowercase().as_str(),
            "en" | "english"
                | "fr"
                | "french"
                | "de"
                | "german"
                | "es"
                | "spanish"
                | "it"
                | "italian"
                | "pt"
                | "portuguese"
                | "ru"
                | "russian"
        );
        let (sendable, passthrough): (Vec<&TranslationEntry>, Vec<&TranslationEntry>) = entries
            .iter()
            .partition(|e| !e.text.is_ascii() || latin_source);

        for (batch_index, chunk) in sendable.chunks(self.batch_size.max(1)).enumerate() {
            let (mut translated, report) =
                self.translate_chunk(chunk, source_lang, target_lang, batch_index)?;
            out.append(&mut translated);
            reports.push(report);
        }
        for entry in passthrough {
            out.push((*entry).clone());
            reports.push(BatchOutput {
                batch: usize::MAX,
                requested: 1,
                translated: 0,
                unchanged: 1,
                extra_ids: 0,
            });
        }
        out.sort_by_key(|e| e.id);
        Ok((out, reports))
    }

    fn translate_chunk(
        &self,
        chunk: &[&TranslationEntry],
        source_lang: &str,
        target_lang: &str,
        batch_index: usize,
    ) -> Result<(Vec<TranslationEntry>, BatchOutput)> {
        let system_prompt = format!(
            "You are a professional video-game script translator. Translate each line from {source_lang} to {target_lang}.\n\
             Rules:\n\
             - Match the register: narration is literary, dialogue is spoken language.\n\
             - Keep lines one-to-one: never merge, split, add, or drop lines.\n\
             - Keep names and honorifics consistent with the glossary when given.\n\
             - Leave placeholders such as {{...}}, %s, control tokens, and file names exactly as they are.\n\
             - Translate meaning over literalness; the result must read like a released game localization.\n\
             - Reply with ONLY a JSON object: {{\"translations\":[{{\"id\":<int>,\"speaker\":<string or null>,\"text\":<translated string>}}]}} \
               containing every input id, in the same order. No markdown, no commentary.{}",
            self.glossary.prompt_fragment()
        );
        let user_payload: Vec<serde_json::Value> = chunk
            .iter()
            .map(|e| {
                let mut v = serde_json::json!({"id": e.id, "text": e.text});
                if let Some(speaker) = &e.speaker {
                    v["speaker"] = serde_json::Value::String(speaker.clone());
                }
                v
            })
            .collect();
        let user_payload = serde_json::Value::Array(user_payload).to_string();

        let body = serde_json::json!({
            "model": self.model,
            "temperature": self.temperature,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": user_payload},
            ],
        });

        let url = format!("{}/chat/completions", self.api_base.trim_end_matches('/'));
        let mut last_error = String::new();
        for attempt in 1..=self.max_attempts.max(1) {
            let mut request = self.agent.post(&url);
            if !self.api_key.is_empty() {
                request = request.set("Authorization", &format!("Bearer {}", self.api_key));
            }
            // Map each fallible step to a String right away: carrying the
            // transport's own error type would balloon this Result.
            let response = match request.send_json(body.clone()) {
                Ok(response) => response
                    .into_json::<serde_json::Value>()
                    .map_err(|e| format!("HTTP: malformed JSON reply: {e}")),
                Err(e) => Err(format!("HTTP: {e}")),
            };

            match response {
                Ok(value) => match Self::parse_reply(&value, chunk) {
                    Ok(result) => return Ok(result),
                    Err(e) => last_error = format!("reply: {e}"),
                },
                Err(e) => last_error = e,
            }
            if attempt < self.max_attempts.max(1) {
                std::thread::sleep(Duration::from_millis(500 * attempt as u64));
            }
        }
        Err(Error::Plugin(format!(
            "{}: batch {} failed after {} attempt(s): {}",
            self.name(),
            batch_index,
            self.max_attempts.max(1),
            last_error
        )))
    }

    /// Pull `{"translations": [...]}` out of a chat completion.
    fn parse_reply(
        value: &serde_json::Value,
        chunk: &[&TranslationEntry],
    ) -> Result<(Vec<TranslationEntry>, BatchOutput)> {
        let content = value
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::corrupt("LLM reply", "no message content in completion"))?;
        let content = strip_json_fences(content);
        let parsed: serde_json::Value = serde_json::from_str(content)
            .map_err(|e| Error::corrupt("LLM reply", format!("not valid JSON: {e}")))?;
        let translations = parsed
            .get("translations")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::corrupt("LLM reply", "missing the \"translations\" array"))?;

        let mut by_id: HashMap<usize, (Option<String>, String)> = HashMap::new();
        for item in translations {
            let id = item
                .get("id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| Error::corrupt("LLM reply", "translation item without an id"))?;
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::corrupt("LLM reply", "translation item without text"))?
                .to_string();
            let speaker = item
                .get("speaker")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            by_id.insert(id as usize, (speaker, text));
        }

        let expected: Vec<usize> = chunk.iter().map(|e| e.id).collect();
        let mut out = Vec::with_capacity(chunk.len());
        let mut translated = 0usize;
        let mut unchanged = 0usize;
        for entry in chunk {
            match by_id.remove(&entry.id) {
                Some((speaker, text)) => {
                    translated += if text != entry.text { 1 } else { 0 };
                    unchanged += if text == entry.text { 1 } else { 0 };
                    out.push(TranslationEntry {
                        id: entry.id,
                        speaker: speaker.or_else(|| entry.speaker.clone()),
                        text,
                    });
                }
                None => {
                    unchanged += 1;
                    out.push((*entry).clone());
                }
            }
        }
        let report = BatchOutput {
            batch: usize::MAX,
            requested: expected.len(),
            translated,
            unchanged,
            extra_ids: by_id.len(),
        };
        Ok((out, report))
    }
}

impl Translator for LlmTranslator {
    fn name(&self) -> &str {
        "llm"
    }

    fn translate(
        &self,
        entries: &[TranslationEntry],
        source_lang: &str,
        target_lang: &str,
    ) -> Result<Vec<TranslationEntry>> {
        Ok(self
            .translate_with_report(entries, source_lang, target_lang)?
            .0)
    }
}

/// Strip ``` / ```json fences some models insist on adding.
fn strip_json_fences(raw: &str) -> &str {
    let trimmed = raw.trim();
    let without_prefix = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .trim();
    without_prefix
        .strip_suffix("```")
        .unwrap_or(without_prefix)
        .trim()
}

/// A deterministic [`Translator`] for tests and offline dry runs.
///
/// Wraps every text as `{marker}{text}` — enough to prove the pipeline
/// moves the right lines to the right places without any network.
#[derive(Clone, Debug)]
pub struct MockTranslator {
    /// Prefix applied to every translated text.
    pub marker: String,
}

impl MockTranslator {
    /// A mock that prefixes `marker`.
    pub fn new(marker: impl Into<String>) -> Self {
        Self {
            marker: marker.into(),
        }
    }
}

impl Translator for MockTranslator {
    fn name(&self) -> &str {
        "mock"
    }

    fn translate(
        &self,
        entries: &[TranslationEntry],
        _source_lang: &str,
        _target_lang: &str,
    ) -> Result<Vec<TranslationEntry>> {
        Ok(entries
            .iter()
            .map(|e| TranslationEntry {
                id: e.id,
                speaker: e.speaker.clone(),
                text: format!("{}{}", self.marker, e.text),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: usize, text: &str) -> TranslationEntry {
        TranslationEntry {
            id,
            speaker: None,
            text: text.into(),
        }
    }

    #[test]
    fn mock_translates_every_line_in_order() {
        let mock = MockTranslator::new("[EN] ");
        let out = mock
            .translate(
                &[entry(0, "こんにちは。"), entry(1, "金継ぎ。")],
                "ja",
                "en",
            )
            .unwrap();
        assert_eq!(out[0].text, "[EN] こんにちは。");
        assert_eq!(out[1].text, "[EN] 金継ぎ。");
    }

    #[test]
    fn fences_are_stripped() {
        assert_eq!(strip_json_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_json_fences("```\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_json_fences("  {\"a\":1}  "), "{\"a\":1}");
    }

    #[test]
    fn parse_reply_maps_ids_and_reports_extras() {
        let e0 = entry(0, "一");
        let e1 = entry(1, "二");
        let chunk = [&e0, &e1];
        let reply = serde_json::json!({
            "choices": [{
                "message": {"content": "{\"translations\":[{\"id\":0,\"speaker\":null,\"text\":\"one\"},{\"id\":1,\"speaker\":null,\"text\":\"two\"},{\"id\":7,\"speaker\":null,\"text\":\"hallucination\"}]}"}
            }]
        });
        let (out, report) = LlmTranslator::parse_reply(&reply, &chunk).unwrap();
        assert_eq!(out[0].text, "one");
        assert_eq!(out[1].text, "two");
        assert_eq!(report.translated, 2);
        assert_eq!(report.extra_ids, 1);
    }

    #[test]
    fn parse_reply_keeps_originals_for_missing_ids() {
        let e0 = entry(0, "一");
        let e1 = entry(1, "二");
        let chunk = [&e0, &e1];
        let reply = serde_json::json!({
            "choices": [{
                "message": {"content": "{\"translations\":[{\"id\":1,\"speaker\":null,\"text\":\"two\"}]}"}
            }]
        });
        let (out, report) = LlmTranslator::parse_reply(&reply, &chunk).unwrap();
        assert_eq!(out[0].text, "一"); // missed by the model → untouched
        assert_eq!(out[1].text, "two");
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.translated, 1);
    }

    #[test]
    fn parse_reply_rejects_non_json() {
        let e0 = entry(0, "一");
        let chunk = [&e0];
        let reply = serde_json::json!({
            "choices": [{"message": {"content": "Sure! Here you go: ..."}}]
        });
        assert!(LlmTranslator::parse_reply(&reply, &chunk).is_err());
    }

    #[test]
    fn glossary_lands_in_the_prompt() {
        let glossary = Glossary::new()
            .term("藍子", "Aiko")
            .term("金継ぎ", "kintsugi");
        assert!(glossary.prompt_fragment().contains("藍子 → Aiko"));
        assert!(Glossary::new().prompt_fragment().is_empty());
    }

    #[test]
    fn ascii_lines_in_japanese_source_bypass_the_model() {
        // No network in tests: if the ASCII filter works, this never dials out.
        let translator = LlmTranslator::new("http://127.0.0.1:1/v1", "", "no-model");
        let out = translator
            .translate(
                &[entry(0, "SND_PLAY ro-mon.ogg"), entry(1, "PLAY_BGM 3")],
                "ja",
                "en",
            )
            .unwrap();
        assert_eq!(out[0].text, "SND_PLAY ro-mon.ogg");
        assert_eq!(out[1].text, "PLAY_BGM 3");
    }
}
