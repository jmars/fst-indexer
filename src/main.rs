use anyhow::Result;
use clap::{Parser, Subcommand};
use fst_indexer::{ExtractorType, Index, SearchOpts};
use std::path::PathBuf;

// ----- CLI -----

#[derive(Parser)]
#[command(name = "fst-indexer", version)]
#[command(
    about = "FST-based full-text indexer — build and search indexes over text, JSONL, and transcript files"
)]
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
        any: bool,
        #[arg(long, default_value = "20")]
        max: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Build {
            dir,
            pattern,
            extractor,
            output,
        } => Index::build(&dir, &pattern, &extractor, &output)?,
        Command::Search {
            index_dir,
            query,
            any,
            max,
        } => {
            let index = Index::open(&index_dir)?;
            let opts = SearchOpts {
                max_results: max,
                any,
            };
            let hits = index.search(&query, &opts)?;

            let results: Vec<serde_json::Value> = hits
                .iter()
                .map(|h| {
                    let entry = index.resolve(h);
                    serde_json::json!({
                        "file_idx": h.file_idx,
                        "entry_idx": h.entry_idx,
                        "filename": entry.map(|e| e.filename.as_str()).unwrap_or("?"),
                        "title": entry.map(|e| e.title.as_str()).unwrap_or("?"),
                        "date": entry.map(|e| e.date.as_str()).unwrap_or("?"),
                        "source": entry.map(|e| e.source.as_str()).unwrap_or("?"),
                    })
                })
                .collect();

            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "query": query,
                    "total_matches": hits.len(),
                    "results": results,
                }))?
            );
        }
    }
    Ok(())
}
