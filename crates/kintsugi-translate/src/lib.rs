//! 🏺 Glaze layer №2: LLM-powered script translation.
//!
//! 大模型翻译: old games are walls of text, and walls of text deserve
//! better than regex-era machine translation. This glaze works on the
//! *engine-agnostic* script IR, so it translates for every seam — today's
//! BlueGale BDT scripts, tomorrow's `kintsugi-kid` — through one pipeline:
//!
//! 1. [`extract`] the text-bearing commands of a [`Script`] into
//!    [`TranslationEntry`] batches (stable ids = command indexes);
//! 2. [`write_jsonl`] them for manual or offline translation — the JSONL
//!    is the interchange format, so humans and other tools can join in;
//! 3. [`Translator::translate`] through any backend — the built-in
//!    [`LlmTranslator`] speaks the OpenAI-compatible chat API (OpenAI,
//!    DeepSeek, vLLM, Ollama, LM Studio…) with a glossary for names and a
//!    strict-JSON contract; [`MockTranslator`] proves the pipeline in tests;
//! 4. [`apply`] the results back onto a copy of the script — text-bearing
//!    commands are replaced, structure untouched, provenance recorded as
//!    warnings so the repair stays visible.
//!
//! The glaze never writes into game files: translated scripts are new
//! artifacts, and the originals stay gold-leafed but intact.

mod entry;
mod translator;

pub use entry::{TranslationEntry, apply, extract, extract_with_raw, read_jsonl, write_jsonl};
pub use translator::{BatchOutput, Glossary, LlmTranslator, MockTranslator, Translator};
