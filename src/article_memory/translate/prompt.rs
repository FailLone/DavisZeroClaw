pub(super) const SYSTEM: &str =
    "You are a careful Chinese (Simplified) translator for technical articles. \
Preserve code blocks, URLs, inline code, and proper nouns verbatim. Do not \
summarize. Do not add commentary. Output only the translated Markdown.";

pub(super) fn user_block(markdown: &str) -> String {
    format!("Translate the following Markdown to Simplified Chinese:\n\n{markdown}")
}
