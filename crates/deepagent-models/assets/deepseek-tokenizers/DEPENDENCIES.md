# Dependency license review (2026-11-14)

The tokenizer JSON is MIT with its preserved `LICENSE`. The official `deepseek-recipe` and `deepseek-recipe-encoding` crates are pinned to 0.2.0 (MIT); `tokenizers` is pinned to 0.23.2 (Apache-2.0). These are unmodified registry crates, not vendored implementations. Cargo.lock contains their registry checksums. Their transitive HTTP/Hub feature is compiled but is not invoked by the offline adapter.

The Windows candidate metadata contains 65 newly selected package versions (including four updates of existing packages). SPDX expressions include MIT, Apache-2.0, Zlib, MIT-0 and the three additional licenses below; do not describe all transitive packages as MIT or treat this list as a legal certification. Other-platform builds and a release's complete dependency notices must be checked with that release's resolved graph.

| Package | License / preserved text | Source archive |
|---|---|---|
| option-ext 0.2.0, via dirs-sys / Hub | [MPL-2.0](option-ext-0.2.0.MPL-2.0.txt) | [unmodified registry source](https://crates.io/api/v1/crates/option-ext/0.2.0/download) |
| webpki-roots 0.26.11, via Hub | [CDLA-Permissive-2.0](webpki-roots-0.26.11.CDLA-Permissive-2.0.txt) | [registry source and data](https://crates.io/api/v1/crates/webpki-roots/0.26.11/download) |
| xxhash-rust 0.8.19, via recipe | [BSL-1.0](xxhash-rust-0.8.19.BSL-1.0.txt) | [registry source](https://crates.io/api/v1/crates/xxhash-rust/0.8.19/download) |

No dependency source is modified here. When distributing executable artifacts, retain applicable copyright/license notices and satisfy the covered-source availability/recipient-notice requirements in MPL-2.0 section 3.2; the above versioned source link identifies the unmodified package. CDLA-Permissive-2.0 section 2.1 requires its text with shared data. Keep these notices in source/package handoffs; local compilation/tests are not verification that an installer or another downstream binary distribution includes them. No installer publication is performed by this change.
