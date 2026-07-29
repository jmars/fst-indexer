# fst-indexer

[![CI](https://github.com/jmars/fst-indexer/actions/workflows/ci.yml/badge.svg)](https://github.com/jmars/fst-indexer/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**FST-based full-text indexer** — blazing fast search over text, JSONL, and transcript files using finite state transducers.

## Quick Start

```bash
cargo install --git https://github.com/jmars/fst-indexer
```

### Build an Index

```bash
# Index all .txt files in a directory as plain text lines
fst-indexer build --dir ./documents --pattern "*.txt" --extractor txt --output ./my-index

# Index .jsonl files (uses the "content" JSON field, falls back to raw line)
fst-indexer build --dir ./logs --pattern "*.jsonl" --extractor jsonl --output ./my-index

# Index Tactiq-format transcript .txt files
fst-indexer build --dir ./transcripts --pattern "*.txt" --extractor transcript --output ./my-index
```

### Search

```bash
# AND search (default): only entries matching ALL query words
fst-indexer search -i ./my-index "deployment failed" --max 10

# OR search: entries matching ANY query word
fst-indexer search -i ./my-index "deployment failed" --any --max 10
```

Results are emitted as JSON:

```json
{
  "query": "deployment failed",
  "total_matches": 5,
  "results": [
    {
      "file_idx": 3,
      "entry_idx": 42,
      "filename": "build-log.jsonl",
      "title": "build-log.jsonl",
      "date": "2024-01-15",
      "source": "jsonl"
    }
  ]
}
```

## How It Works

1. **Extractors** read source files and produce a list of text entries (lines, turns, etc.).
2. **Tokenization** splits each entry into lowercase words (2-100 characters, alphanumeric + `_` + `-`).
3. **Keys** are constructed as `{word}\0{file_idx:u32}{entry_idx:u32}` and inserted into an FST set via `fst::SetBuilder`.
4. **Search** tokenizes the query, prefix-scans the FST for each word, and intersects (AND) or unions (OR) the hit sets.
5. Results are returned as pure `(file_idx, entry_idx)` pairs — no re-reading of source files.

## Architecture

The codebase is a single crate with two source files:

- **`src/lib.rs`** — The public API. Exposes the `Index` type (build, open, search, resolve), along with `Hit`, `SearchOpts`, `ExtractorType`, and `FileEntry`. Also contains all extractors (txt, jsonl, transcript), the tokenizer, and the test suite.
- **`src/main.rs`** — The CLI entry point. Uses `clap` for argument parsing and delegates to `Index::build` / `Index::search`.

### Key types

| Type | Description |
|------|-------------|
| `Index` | The main type — holds an FST set and a manifest. Call `Index::open()` to load, then `search()` and `resolve()`. |
| `Hit` | A `(file_idx, entry_idx)` pair pointing to a matched entry. |
| `SearchOpts` | Controls search behavior: `max_results` (default 20) and `any` (OR mode vs AND mode). |
| `ExtractorType` | Enum selecting the extractor: `Txt`, `Jsonl`, or `Transcript`. |
| `FileEntry` | Metadata for an indexed file: `filename`, `title`, `date`, `source`. |

### Extractors

Each extractor implements the same pattern: read a file, parse its content, return a list of text entries plus a `FileEntry` with metadata. The **txt** extractor treats each line as an entry. The **jsonl** extractor parses each JSON line and uses the `"content"` field if present. The **transcript** extractor parses Tactiq-format speaker turns.

## API Usage

You can use `fst-indexer` as a Rust library dependency in your own project:

```toml
[dependencies]
fst-indexer = { git = "https://github.com/jmars/fst-indexer" }
```

```rust
use fst_indexer::{Index, SearchOpts};

let index = Index::open("path/to/index")?;
let hits = index.search("query", &SearchOpts::default())?;
for hit in &hits {
    if let Some(entry) = index.resolve(hit) {
        println!("{}:{} in {}", entry.filename, hit.entry_idx, entry.date);
    }
}
```

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for setup instructions, development workflow, and pull request guidelines.

This project is licensed under the MIT License — all contributions are accepted under the same license.

## License

MIT
