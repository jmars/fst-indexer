# fst-indexer

**FST-based full-text indexer** — blazing fast search over text, JSONL, and transcript files using finite state transducers.

## Quick Start

```bash
cargo install fst-indexer
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
    {"file_idx": 3, "entry_idx": 42},
    {"file_idx": 1, "entry_idx": 17}
  ]
}
```

## How It Works

1. **Extractors** read source files and produce a list of text entries (lines, turns, etc.).
2. **Tokenization** splits each entry into lowercase words (2-100 characters, alphanumeric + `_` + `-`).
3. **Keys** are constructed as `{word}\0{file_idx:u32}{entry_idx:u32}` and inserted into an FST set via `fst::SetBuilder`.
4. **Search** tokenizes the query, prefix-scans the FST for each word, and intersects (AND) or unions (OR) the hit sets.
5. Results are returned as pure `(file_idx, entry_idx)` pairs — no re-reading of source files.

## License

MIT
