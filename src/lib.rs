#![allow(clippy::module_name_repetitions)]

use anyhow::{Context, Result};
use clap::ValueEnum;
use fst::{IntoStreamer, Streamer};
use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

// ----- Public types -----

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEntry {
    pub filename: String,
    pub title: String,
    pub date: String,
    pub source: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub file_idx: u32,
    pub entry_idx: u32,
}

#[derive(Debug, Clone)]
pub struct SearchOpts {
    pub max_results: usize,
    pub any: bool,
}

impl Default for SearchOpts {
    fn default() -> Self {
        Self {
            max_results: 20,
            any: false,
        }
    }
}

#[derive(Clone, Debug, ValueEnum)]
pub enum ExtractorType {
    /// JSONL files: each JSON line is parsed; 'content' field used if present, else whole line
    Jsonl,
    /// Plain text files: each line is one entry
    Txt,
    /// Tactiq transcript .txt files: speaker turns parsed and indexed
    Transcript,
}

pub struct Index {
    fst: fst::Set<Vec<u8>>,
    manifest: Manifest,
}

// ----- Internal types -----

#[derive(Debug)]
#[allow(dead_code)]
struct Turn {
    speaker: String,
    timestamp: String,
    text: String,
}

struct TranscriptData {
    meeting: Option<String>,
    meeting_date: Option<String>,
    turns: Vec<Turn>,
}

// ----- Index impl -----

impl Index {
    pub fn build(
        dir: &Path,
        pattern: &str,
        extractor: &ExtractorType,
        output: &Path,
    ) -> Result<()> {
        std::fs::create_dir_all(output)
            .with_context(|| format!("Creating output directory {}", output.display()))?;

        let fst_path = output.join("index.fst");
        let manifest_path = output.join("manifest.json");

        let files = collect_files(dir, pattern).context("Collecting source files")?;

        eprintln!("Found {} files matching pattern '{}'", files.len(), pattern);

        let mut entries: Vec<FileEntry> = Vec::new();
        let mut keys = BTreeSet::new();
        let mut total_entries = 0u64;

        for (file_idx, filepath) in files.iter().enumerate() {
            let file_idx = u32::try_from(file_idx).expect("too many files");
            let filename = filepath
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();

            let (file_entry, extracted_entries) = match extractor {
                ExtractorType::Jsonl => extract_jsonl(filepath, &filename)?,
                ExtractorType::Txt => extract_txt(filepath, &filename)?,
                ExtractorType::Transcript => extract_transcript(filepath, &filename)?,
            };

            entries.push(file_entry);

            for (entry_idx, text) in extracted_entries.iter().enumerate() {
                let entry_idx = u32::try_from(entry_idx).expect("too many entries");
                total_entries += 1;

                for word in tokenize(text) {
                    let mut k = word.into_bytes();
                    k.push(0);
                    k.extend_from_slice(&file_idx.to_be_bytes());
                    k.extend_from_slice(&entry_idx.to_be_bytes());
                    keys.insert(k);
                }
            }
        }

        eprintln!(
            "{} unique keys from {} entries across {} files",
            keys.len(),
            total_entries,
            entries.len()
        );

        write_fst(&fst_path, &keys)?;
        write_manifest(&manifest_path, entries)?;
        report_size(&fst_path, &manifest_path);

        Ok(())
    }

    pub fn open(index_dir: &Path) -> Result<Self> {
        let fst_path = index_dir.join("index.fst");
        let manifest_path = index_dir.join("manifest.json");

        if !manifest_path.exists() {
            anyhow::bail!("No manifest.json found in {}", index_dir.display());
        }

        let fst_bytes =
            std::fs::read(&fst_path).with_context(|| format!("Opening {}", fst_path.display()))?;
        let fst = fst::Set::new(fst_bytes).context("Creating FST set from bytes")?;

        let manifest_str = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("Reading {}", manifest_path.display()))?;
        let manifest: Manifest =
            serde_json::from_str(&manifest_str).context("Parsing manifest.json")?;

        Ok(Index { fst, manifest })
    }

    pub fn search(&self, query: &str, opts: &SearchOpts) -> Result<Vec<Hit>> {
        let query_words: Vec<String> = tokenize(query);
        if query_words.is_empty() {
            return Ok(Vec::new());
        }

        type HitSet = HashSet<(u32, u32)>;
        let mut word_sets: Vec<HitSet> = Vec::new();

        for word in &query_words {
            let mut prefix = word.as_bytes().to_vec();
            prefix.push(0);
            let mut hits = HitSet::new();
            let mut stream = self.fst.range().ge(prefix.as_slice()).into_stream();
            while let Some(kb) = stream.next() {
                if kb.len() < prefix.len() + 8 {
                    continue;
                }
                if !kb.starts_with(&prefix) {
                    break;
                }
                let payload = &kb[prefix.len()..];
                let fi = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let ei = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
                hits.insert((fi, ei));
            }
            // In AND mode, if any word has no hits, the intersection is empty
            if hits.is_empty() && !opts.any {
                return Ok(Vec::new());
            }
            if !hits.is_empty() {
                word_sets.push(hits);
            }
        }

        if word_sets.is_empty() {
            return Ok(Vec::new());
        }

        let all_hits: Vec<Hit> = if opts.any {
            // OR mode: union all word results
            let mut all = HitSet::new();
            for ws in &word_sets {
                all.extend(ws);
            }
            let mut hits: Vec<Hit> = all
                .into_iter()
                .map(|(f, e)| Hit {
                    file_idx: f,
                    entry_idx: e,
                })
                .collect();
            hits.sort_by(|a, b| {
                b.file_idx
                    .cmp(&a.file_idx)
                    .then(a.entry_idx.cmp(&b.entry_idx))
            });
            hits.truncate(opts.max_results);
            hits
        } else {
            // AND mode: intersection
            let mut all = word_sets[0].clone();
            for ws in &word_sets[1..] {
                all = all.intersection(ws).copied().collect();
            }
            let mut hits: Vec<Hit> = all
                .into_iter()
                .map(|(f, e)| Hit {
                    file_idx: f,
                    entry_idx: e,
                })
                .collect();
            hits.sort_by(|a, b| {
                b.file_idx
                    .cmp(&a.file_idx)
                    .then(a.entry_idx.cmp(&b.entry_idx))
            });
            hits.truncate(opts.max_results);
            hits
        };

        Ok(all_hits)
    }

    pub fn resolve(&self, hit: &Hit) -> Option<&FileEntry> {
        self.manifest.files.get(usize::try_from(hit.file_idx).ok()?)
    }
}

// ----- Extractor helpers -----

fn extract_jsonl(path: &Path, filename: &str) -> Result<(FileEntry, Vec<String>)> {
    let date = date_from_path(path);
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping non-UTF-8 file {}: {}", path.display(), e);
            return Ok((
                FileEntry {
                    filename: filename.to_string(),
                    title: filename.to_string(),
                    date,
                    source: "jsonl".into(),
                },
                vec![],
            ));
        }
    };
    let mut entries = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let text = match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => v
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or(line)
                .to_string(),
            Err(_) => line.to_string(),
        };
        entries.push(text);
    }

    Ok((
        FileEntry {
            filename: filename.to_string(),
            title: filename.to_string(),
            date,
            source: "jsonl".into(),
        },
        entries,
    ))
}

fn extract_txt(path: &Path, filename: &str) -> Result<(FileEntry, Vec<String>)> {
    let date = date_from_path(path);
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping non-UTF-8 file {}: {}", path.display(), e);
            return Ok((
                FileEntry {
                    filename: filename.to_string(),
                    title: filename.to_string(),
                    date,
                    source: "txt".into(),
                },
                vec![],
            ));
        }
    };
    let entries: Vec<String> = content.lines().map(ToString::to_string).collect();

    Ok((
        FileEntry {
            filename: filename.to_string(),
            title: filename.to_string(),
            date,
            source: "txt".into(),
        },
        entries,
    ))
}

fn extract_transcript(path: &Path, filename: &str) -> Result<(FileEntry, Vec<String>)> {
    // Reject binary files (e.g., .docx) — transcripts must be valid UTF-8 text
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Skipping non-UTF-8 file {}: {}", path.display(), e);
            return Ok((
                FileEntry {
                    filename: filename.to_string(),
                    title: filename.to_string(),
                    date: date_from_path(path),
                    source: "transcript".into(),
                },
                vec![],
            ));
        }
    };
    let parsed = parse_transcript(&content);

    let date = parsed
        .meeting_date
        .clone()
        .unwrap_or_else(|| date_from_path(path));
    let title = parsed
        .meeting
        .clone()
        .unwrap_or_else(|| filename.to_string());

    // Each turn produces one entry combining speaker name and text
    let entries: Vec<String> = parsed
        .turns
        .iter()
        .map(|t| format!("{}: {}", t.speaker, t.text))
        .collect();

    Ok((
        FileEntry {
            filename: filename.to_string(),
            title,
            date,
            source: "transcript".into(),
        },
        entries,
    ))
}

// ----- Transcript parser -----

fn parse_transcript(text: &str) -> TranscriptData {
    let lines: Vec<&str> = text.lines().collect();
    let mut meeting = None;
    let mut meeting_date = None;
    let mut turns = Vec::new();
    let header_end = lines
        .iter()
        .position(|l| l.trim().starts_with("==="))
        .unwrap_or(0);

    for line in &lines[..header_end] {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("Meeting:") {
            meeting = Some(v.trim().to_string());
        }
        if let Some(v) = l.strip_prefix("Date:") {
            if meeting_date.is_none() {
                let v = v.trim();
                meeting_date = Some(if v.len() >= 10 {
                    v[..10].to_string()
                } else {
                    v.to_string()
                });
            }
        }
    }

    let mut cur_speaker = String::new();
    let mut cur_ts = String::new();
    let mut cur_text = Vec::new();

    for line in lines[header_end..].iter() {
        if let Some(cap) = line.trim().strip_suffix(')') {
            if let Some(idx) = cap.rfind(" (") {
                let name = cap[..idx].trim().to_string();
                let ts = cap[idx + 2..].trim().to_string();
                if ts.len() == 8 && ts.contains(':') {
                    if !cur_speaker.is_empty() {
                        turns.push(Turn {
                            speaker: cur_speaker,
                            timestamp: cur_ts,
                            text: cur_text.join("\n").trim().to_string(),
                        });
                    }
                    cur_speaker = name;
                    cur_ts = ts;
                    cur_text.clear();
                    continue;
                }
            }
        }
        if !cur_speaker.is_empty() {
            cur_text.push(line.to_string());
        }
    }
    if !cur_speaker.is_empty() {
        turns.push(Turn {
            speaker: cur_speaker,
            timestamp: cur_ts,
            text: cur_text.join("\n").trim().to_string(),
        });
    }

    TranscriptData {
        meeting,
        meeting_date,
        turns,
    }
}

// ----- Shared helpers -----

fn write_fst(path: &Path, keys: &BTreeSet<Vec<u8>>) -> Result<()> {
    let wtr = std::io::BufWriter::new(
        std::fs::File::create(path).with_context(|| format!("Creating {}", path.display()))?,
    );
    let mut builder = fst::SetBuilder::new(wtr).context("Creating FST builder")?;
    for key in keys {
        builder.insert(key).context("Inserting key")?;
    }
    builder.finish().context("Finishing FST")?;
    Ok(())
}

fn write_manifest(path: &Path, entries: Vec<FileEntry>) -> Result<()> {
    let manifest = Manifest { files: entries };
    std::fs::write(path, serde_json::to_string_pretty(&manifest)?)
        .with_context(|| format!("Writing {}", path.display()))?;
    Ok(())
}

fn report_size(fst_path: &Path, manifest_path: &Path) {
    let fs = std::fs::metadata(fst_path).map(|m| m.len()).unwrap_or(0);
    let ms = std::fs::metadata(manifest_path)
        .map(|m| m.len())
        .unwrap_or(0);
    eprintln!(
        "Done. FST: {:.2} MB, Manifest: {:.1} KB",
        fs as f64 / 1_048_576.0,
        ms as f64 / 1024.0
    );
}

fn collect_files(dir: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let pat =
        Pattern::new(pattern).with_context(|| format!("Invalid glob pattern '{}'", pattern))?;
    let mut files = Vec::new();
    collect_files_recursive(dir, &pat, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_recursive(dir: &Path, pat: &Pattern, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("Reading directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Skip symlinked directories to avoid cycles and unintended traversal
            if path.symlink_metadata()?.file_type().is_symlink() {
                eprintln!("Skipping symlinked directory: {}", path.display());
                continue;
            }
            collect_files_recursive(&path, pat, files)?;
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if pat.matches(name) {
                files.push(path);
            }
        }
    }
    Ok(())
}

fn date_from_path(path: &Path) -> String {
    // Extract YYYY-MM-DD from the filename if present
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let bytes = name.as_bytes();
        for i in 0..bytes.len().saturating_sub(9) {
            if is_date_prefix(&bytes[i..]) {
                return name[i..i + 10].to_string();
            }
        }
    }
    "?".to_string()
}

fn is_date_prefix(bytes: &[u8]) -> bool {
    bytes.len() >= 10
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[5].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7] == b'-'
        && bytes[8].is_ascii_digit()
        && bytes[9].is_ascii_digit()
}

/// Tokenize text into lowercase word tokens (length 2-100, alphanumeric/underscore/hyphen).
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|s| s.to_lowercase())
        .filter(|s| s.len() >= 2 && s.len() <= 100)
        .collect()
}

// ----- Tests -----

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("Hello World");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_tokenize_short_words_skipped() {
        let tokens = tokenize("a an the cat");
        // "a" is len 1 (filtered), "an" is len 2 (included), "the"/"cat" are >= 2
        assert_eq!(tokens, vec!["an", "the", "cat"]);
    }

    #[test]
    fn test_tokenize_hyphen_and_underscore() {
        let tokens = tokenize("multi-word and_snake_case");
        assert_eq!(tokens, vec!["multi-word", "and_snake_case"]);
    }

    #[test]
    fn test_is_date_prefix_valid() {
        assert!(is_date_prefix(b"2024-01-15"));
    }

    #[test]
    fn test_is_date_prefix_invalid() {
        assert!(!is_date_prefix(b"2024/01/15"));
        assert!(!is_date_prefix(b"short"));
    }

    #[test]
    fn test_date_from_path_matches() {
        let p = Path::new("/some/logs/2024-01-15_events.jsonl");
        assert_eq!(date_from_path(p), "2024-01-15");
    }

    #[test]
    fn test_date_from_path_no_match() {
        let p = Path::new("/some/logs/no_date_here.jsonl");
        assert_eq!(date_from_path(p), "?");
    }

    #[test]
    fn test_build_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = tempfile::tempdir().unwrap();
        let output = index_dir.path().join("test_index");

        // Write a test TXT file
        let src = dir.path().join("test-2024-01-15.txt");
        let mut f = std::fs::File::create(&src).unwrap();
        writeln!(f, "hello world").unwrap();
        writeln!(f, "foo bar").unwrap();
        writeln!(f, "hello again").unwrap();

        // Build index
        Index::build(dir.path(), "*.txt", &ExtractorType::Txt, &output).unwrap();

        // Open and search
        let index = Index::open(&output).unwrap();

        // AND search
        let hits = index.search("hello", &SearchOpts::default()).unwrap();
        assert_eq!(hits.len(), 2);

        let hits = index.search("hello world", &SearchOpts::default()).unwrap();
        assert_eq!(hits.len(), 1);

        // OR search: union of all word matches (deduplicated)
        let opts = SearchOpts {
            any: true,
            ..Default::default()
        };
        let hits = index.search("hello world", &opts).unwrap();
        // "hello" hits lines 0 and 2, "world" hits line 0 → union = 2 unique hits
        assert_eq!(hits.len(), 2);

        // Resolve
        let entry = index.resolve(&hits[0]).unwrap();
        assert_eq!(entry.filename, "test-2024-01-15.txt");
        assert_eq!(entry.date, "2024-01-15");
        assert_eq!(entry.source, "txt");
    }

    #[test]
    fn test_search_empty_query() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("idx");
        let src = dir.path().join("data.txt");
        std::fs::write(&src, "hello world").unwrap();
        Index::build(dir.path(), "*.txt", &ExtractorType::Txt, &output).unwrap();

        let index = Index::open(&output).unwrap();
        let hits = index.search("", &SearchOpts::default()).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn test_search_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("idx");
        let src = dir.path().join("data.txt");
        std::fs::write(&src, "hello world").unwrap();
        Index::build(dir.path(), "*.txt", &ExtractorType::Txt, &output).unwrap();

        let index = Index::open(&output).unwrap();
        let hits = index.search("nonexistent", &SearchOpts::default()).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn test_resolve_out_of_range() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("idx");
        let src = dir.path().join("data.txt");
        std::fs::write(&src, "hello world").unwrap();
        Index::build(dir.path(), "*.txt", &ExtractorType::Txt, &output).unwrap();

        let index = Index::open(&output).unwrap();
        let hit = Hit {
            file_idx: 999,
            entry_idx: 0,
        };
        assert!(index.resolve(&hit).is_none());
    }

    #[test]
    fn test_open_nonexistent_dir() {
        let result = Index::open(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_extractor_default_jsonl_content_field() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("log-2024-01-15.jsonl");
        std::fs::write(&src, r#"{"content": "hello world", "level": "info"}"#).unwrap();
        let output = dir.path().join("idx");
        Index::build(dir.path(), "*.jsonl", &ExtractorType::Jsonl, &output).unwrap();

        let index = Index::open(&output).unwrap();
        let hits = index.search("hello", &SearchOpts::default()).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn test_extractor_jsonl_raw_line_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("log-2024-01-15.jsonl");
        std::fs::write(&src, "raw line without content field\n").unwrap();
        let output = dir.path().join("idx");
        Index::build(dir.path(), "*.jsonl", &ExtractorType::Jsonl, &output).unwrap();

        let index = Index::open(&output).unwrap();
        let hits = index.search("raw", &SearchOpts::default()).unwrap();
        assert_eq!(hits.len(), 1);
    }
}
