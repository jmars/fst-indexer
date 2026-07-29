# Changelog

All notable changes to this project will be documented in this file.

## [0.1.0] — 2026-07-29

### Added
- Initial release: generic FST-based full-text indexer CLI
- `build` command — index text, JSONL, and Tactiq transcript files
- `search` command — AND/OR search with configurable max results
- Three extractors: `txt`, `jsonl`, `transcript`
- Public library API: `Index::build()`, `Index::open()`, `Index::search()`, `Index::resolve()`
- `tokenize()` public function for downstream use
- Manifest-based file metadata resolution in search results
- 14 unit tests covering tokenization, date extraction, build/search/resolve cycle, edge cases

[0.1.0]: https://github.com/jmars/fst-indexer/releases/tag/v0.1.0
