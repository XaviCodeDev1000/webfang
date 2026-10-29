#![no_main]
use libfuzzer_sys::fuzz_target;
use webfang::domain::ports::MarkdownConverter;

// Fuzz HTML-to-Markdown conversion — the single converter pipeline
// (clean_html pre-pass + htmd). Processes untrusted HTML. Panic = DoS on the scraper.
fuzz_target!(|data: &[u8]| {
    if let Ok(html) = std::str::from_utf8(data) {
        let markdown = webfang::infrastructure::converter::htmd_converter::HtmdConverter.convert(html);

        // Sanity: non-empty HTML should produce some output
        if !html.is_empty() && markdown.is_empty() {
            eprintln!("WARN: non-empty HTML produced empty markdown ({} bytes)", html.len());
        }
    }
});
