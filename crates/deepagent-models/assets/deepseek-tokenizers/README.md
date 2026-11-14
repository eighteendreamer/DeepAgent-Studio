# Pinned offline tokenizer data

Source: [DeepSeek recipe tokenizer documentation](https://github.com/deepseek-ai/deepseek-recipe/blob/main/docs/tokenizer.zh.md), retrieved 2026-11-14. Official templates: `deepseek-recipe` / `deepseek-recipe-encoding` **0.2.0**; BPE implementation: Rust `tokenizers` **0.23.2**. Data is embedded as gzip, decompressed once, length/SHA-256 checked before parsing. No runtime download, model request, credential lookup, or image decoding.

| Asset | Raw JSON bytes | Raw SHA-256 |
|---|---:|---|
| v4.json.gz | 6367146 | 97d2f31b020d18b5aee5c9b3d5b4efb10ea210f3fe3f7dffe3f1cd90542d6b19 |
| v41.json.gz | 6367247 | 81f64d1248a68ce3663e07ab3ee48b851e5df0e32d27cb98e4c9a268151e8d99 |

Both compressed assets total 3,727,057 bytes. They reproduce the raw JSON from `https://raw.githubusercontent.com/deepseek-ai/deepseek-recipe/main/static/tokenizers/{v4,v41}/tokenizer.json`. See the preserved `v4.upstream.md` / `v41.upstream.md` for comparison revisions and special-image-token changes; their relative links refer to the upstream layout. The raw SHA, not the mutable URL, defines the accepted data. Keep the accompanying MIT `LICENSE` (Copyright (c) 2023 DeepSeek) on redistribution. Recipe crates are MIT; tokenizers is Apache-2.0.

Local route binding, based on the [official API model table](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/) on 2026-11-14: `deepseek-v4-pro` uses V4; `deepseek-flash`, `deepseek-v4-flash`, `deepseek-v4-flash-vision-exp` use V4.1. The latter two IDs are documented aliases of V4.1-Flash-1017, not guesses from a name substring. The recipe's canonical model names are also accepted. Unknown/future model IDs have no asserted template match.

Counts are **offline estimates**, not a guarantee that a hosted model still uses these exact weights/templates or a billing figure. User-written special-token spellings are plain text. Tool schemas/calls, reasoning, response formats and template control tokens use the official converter/renderer. Hosted web search is outside recipe's supported scope and must not be silently dropped from a measurement. Images use placeholders plus the existing routed image reserve; image bytes/URLs are not text tokens. Actual provider usage and invalidation on request-header/model/encoder changes remain authoritative.

The official encoding crate transitively enables tokenizers' HTTP/Hub dependencies; disabling its features at our direct dependency does not turn those off. This adapter uses only embedded `Tokenizer::from_bytes`; `from_pretrained`, Hub fetches and image fetchers are not used. This adds compiled dependencies but does not introduce a runtime network-loading route.
