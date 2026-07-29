use std::collections::{BTreeSet, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use fst::{IntoStreamer, Streamer};
use glob::Pattern;
use serde::{Deserialize, Serialize};

// ----- CLI -----

#[derive(Parser)]
#[command(name = "fst-indexer")]
#[command(about = "FST-based full-text indexer — build and search indexes over text, JSONL, and transcript files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a full-text index from source files
    Build {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        pattern: String,
        #[arg(long)]
        extractor: ExtractorType,
        #[arg(long)]
        output: PathBuf,
    },
    /// Search an existing index
    Search {
        #[arg(short = 'i', long)]
        index_dir: PathBuf,
        query: String,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        any: bool,
        #[arg(long, default_value = "20")]
        max: usize,
    },
}

#[derive(Clone, ValueEnum)]
enum ExtractorType {
    /// JSONL files: each JSON line is parsed; 'content' field used if present, else whole line
    Jsonl,
    /// Plain text files: each line is one entry
    Txt,
    /// Tactiq transcript .txt files: speaker turns parsed and indexed
    Transcript,
}

// ----- Manifest -----

#[derive(Serialize, Deserialize, Clone, Debug)]
struct FileEntry {
    filename: String,
    title: String,
    date: String,
    source: String,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    files: Vec<FileEntry>,
}

// ---------------------------------------------------------------------------
// BUILD
// ---------------------------------------------------------------------------

fn cmd_build(dir: &Path, pattern: &str, extractor: &ExtractorType, output: &Path) -> Result<()> {
    std::fs::create_dir_all(output)
        .with_context(|| format!("Creating output directory {}", output.display()))?;

    let fst_path = output.join("index.fst");
    let manifest_path = output.join("manifest.json");

    let files = collect_files(dir, pattern)
        .context("Collecting source files")?;

    eprintln!("Found {} files matching pattern '{}'", files.len(), pattern);

    let mut entries: Vec<FileEntry> = Vec::new();
    let mut keys = BTreeSet::new();
    let mut total_entries = 0u64;

    for (file_idx, filepath) in files.iter().enumerate() {
        let file_idx = file_idx as u32;
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
            let entry_idx = entry_idx as u32;
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

// ---------------------------------------------------------------------------
// EXTRACTORS
// ---------------------------------------------------------------------------

fn extract_jsonl(path: &Path, filename: &str) -> Result<(FileEntry, Vec<String>)> {
    let date = date_from_path(path);
    let file = std::fs::File::open(path)
        .with_context(|| format!("Opening {}", path.display()))?;
    let mut entries = Vec::new();

    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let text = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(v) => v
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or(&line)
                .to_string(),
            Err(_) => line,
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
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Reading {}", path.display()))?;
    let entries: Vec<String> = content.lines().map(|l| l.to_string()).collect();

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
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Reading {}", path.display()))?;
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

// ---------------------------------------------------------------------------
// SEARCH
// ---------------------------------------------------------------------------

fn cmd_search(index_dir: &Path, query: &str, max_results: usize, any: bool) -> Result<()> {
    let fst_path = index_dir.join("index.fst");
    let _manifest_path = index_dir.join("manifest.json");

    // Verify manifest exists (catches wrong directory early)
    if !_manifest_path.exists() {
        anyhow::bail!("No manifest.json found in {}", index_dir.display());
    }

    let fst_bytes = std::fs::read(&fst_path)
        .with_context(|| format!("Opening {}", fst_path.display()))?;
    let fst_set = fst::Set::from_bytes(fst_bytes).context("Creating FST set from bytes")?;

    let query_words: Vec<String> = tokenize(query);
    if query_words.is_empty() {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "query": query,
                "total_matches": 0,
                "results": []
            }))?
        );
        return Ok(());
    }

    type HitSet = HashSet<(u32, u32)>;
    let mut word_sets: Vec<HitSet> = Vec::new();

    for word in &query_words {
        let mut prefix = word.as_bytes().to_vec();
        prefix.push(0);
        let mut hits = HitSet::new();
        let mut stream = fst_set.range().ge(prefix.as_slice()).into_stream();
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
            if hits.len() > 10_000 {
                break;
            }
        }
        // In OR mode, skip words with no hits instead of returning empty
        if hits.is_empty() && !any {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "query": query,
                    "total_matches": 0,
                    "results": []
                }))?
            );
            return Ok(());
        }
        if !hits.is_empty() {
            word_sets.push(hits);
        }
    }

    if word_sets.is_empty() {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "query": query,
                "total_matches": 0,
                "results": []
            }))?
        );
        return Ok(());
    }

    let all_hits: Vec<Hit> = if any {
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
        hits.sort_by(|a, b| b.file_idx.cmp(&a.file_idx).then(a.entry_idx.cmp(&b.entry_idx)));
        hits.truncate(max_results);
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
        hits.sort_by(|a, b| b.file_idx.cmp(&a.file_idx).then(a.entry_idx.cmp(&b.entry_idx)));
        hits.truncate(max_results);
        hits
    };

    // Emit pure hits as JSON
    let results: Vec<serde_json::Value> = all_hits
        .iter()
        .map(|h| {
            serde_json::json!({
                "file_idx": h.file_idx,
                "entry_idx": h.entry_idx,
            })
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "query": query,
            "total_matches": all_hits.len(),
            "results": results,
        }))?
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Hit {
    file_idx: u32,
    entry_idx: u32,
}

fn write_fst(path: &Path, keys: &BTreeSet<Vec<u8>>) -> Result<()> {
    let wtr = std::io::BufWriter::new(
        std::fs::File::create(path)
            .with_context(|| format!("Creating {}", path.display()))?,
    );
    let mut builder =
        fst::SetBuilder::new(wtr).context("Creating FST builder")?;
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
    let fs = std::fs::metadata(fst_path)
        .map(|m| m.len())
        .unwrap_or(0);
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
    let pat = Pattern::new(pattern)
        .with_context(|| format!("Invalid glob pattern '{}'", pattern))?;
    let mut files = Vec::new();
    collect_files_recursive(dir, &pat, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_recursive(dir: &Path, pat: &Pattern, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("Reading directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
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

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|s| s.to_lowercase())
        .filter(|s| s.len() >= 2 && s.len() <= 100)
        .collect()
}

// ---------------------------------------------------------------------------
// Transcript parser (unchanged from original)
// ---------------------------------------------------------------------------

#[derive(Debug)]
#[allow(dead_code)]
struct Turn {
    speaker: String,
    timestamp: String,
    text: String,
}

struct Transcript {
    meeting: Option<String>,
    meeting_date: Option<String>,
    turns: Vec<Turn>,
}

fn parse_transcript(text: &str) -> Transcript {
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
                meeting_date = Some(v.trim()[..10].to_string());
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

    Transcript {
        meeting,
        meeting_date,
        turns,
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build {
            dir,
            pattern,
            extractor,
            output,
        } => cmd_build(&dir, &pattern, &extractor, &output)?,
        Command::Search {
            index_dir,
            query,
            json: _,
            any,
            max,
        } => cmd_search(&index_dir, &query, max, any)?,
    }
    Ok(())
}
