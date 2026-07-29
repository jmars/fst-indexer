# Roadmap

## Non-Goals (may be revisited later)

- **Incremental indexing** — the current design rebuilds the full index from scratch. No diff/append support.
- **Docx transcript support** — document formats like .docx are handled by upstream tools; feed their output as .txt.
- **Plug-in extractor system** — extractors are built-in and compile-time for v1. No dynamic loading.
- **Stemming or fuzzy search** — exact word match only. No prefix/suffix stemming or Levenshtein.
- **File watching / automatic rebuild** — indexing is an explicit CLI command. No inotify/kqueue watcher.
- **Rendering or context display** — the search command returns pure `(file_idx, entry_idx)` hits. Downstream tools handle display.
