use anyhow::{Context, Result, ensure};
use cargo_metadata::{
    MetadataCommand,
    diagnostic::{Diagnostic, DiagnosticLevel, DiagnosticSpan},
};
use ra_ap_ide::{TextEdit, TextSize};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    fs::{read_to_string, write},
    path::Path,
    process::Command,
};

mod auto_import;
use auto_import::{FileOffsetMap, auto_import};

const SUFFIX: &str = "_wc";

type FileSpanMap = BTreeMap<String, HashSet<DiagnosticSpan>>;

#[derive(Debug, Deserialize)]
struct Message {
    reason: String,
    #[serde(rename = "message")]
    diagnostic: Option<Diagnostic>,
}

fn main() -> Result<()> {
    let metadata = MetadataCommand::new().no_deps().exec()?;
    eprintln!("Fixing: {}", metadata.workspace_root);

    let diag_spans = collect_diagnostic_spans()?;

    // Clippy can report the same source span more than once, so group by file and
    // deduplicate before constructing edits.
    let mut file_span_map = FileSpanMap::new();

    for diag_span in diag_spans {
        let file_name = diag_span.file_name.clone();
        let diag_spans = file_span_map.entry(file_name).or_default();
        diag_spans.insert(diag_span);
    }

    let file_offset_map = rewrite_symbols(metadata.workspace_root.as_std_path(), file_span_map)?;

    auto_import(metadata.workspace_root.as_std_path(), file_offset_map)?;

    remove_unused_imports()?;

    fmt()?;

    Ok(())
}

pub fn collect_diagnostic_spans() -> Result<Vec<DiagnosticSpan>> {
    let mut command = elaborate::disallowed_methods();
    command.arg("--message-format=json");
    eprintln!("{command:?}");
    let output = command.output()?;

    let mut diag_spans = Vec::new();

    let stdout = String::from_utf8(output.stdout)?;
    for result in serde_json::Deserializer::from_str(&stdout).into_iter::<Message>() {
        let message: Message = result?;
        if message.reason != "compiler-message" {
            continue;
        }
        let Some(diagnostic) = message.diagnostic else {
            continue;
        };
        let Some(code) = diagnostic.code else {
            continue;
        };
        if code.code != "clippy::disallowed_methods" {
            continue;
        }
        if diagnostic.level != DiagnosticLevel::Error {
            eprintln!("diagnostic.level is {:?}", diagnostic.level);
            continue;
        }
        if diagnostic.spans.len() != 1 {
            eprintln!(
                "expected one span, got {}: {:#?}",
                diagnostic.spans.len(),
                &diagnostic.spans
            );
        }
        diag_spans.extend_from_slice(&diagnostic.spans);
    }

    Ok(diag_spans)
}

/// Rewrites the diagnosed symbols and returns cursor positions in the rewritten files.
/// rust-analyzer uses those positions to discover which unresolved names need imports.
fn rewrite_symbols(workspace_root: &Path, file_span_map: FileSpanMap) -> Result<FileOffsetMap> {
    let mut file_offset_map = FileOffsetMap::new();

    for (file_name, diag_spans) in file_span_map {
        let path = workspace_root.join(&file_name);
        let mut diag_spans = diag_spans.into_iter().collect::<Vec<_>>();
        diag_spans.sort_by_key(|diag_span| (diag_span.byte_start, diag_span.byte_end));

        let mut contents = match read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) => {
                eprintln!("{file_name}: {error}");
                continue;
            }
        };
        let mut edit = TextEdit::builder();
        let mut offsets = Vec::with_capacity(diag_spans.len());
        for diag_span in diag_spans {
            edit.insert(TextSize::new(diag_span.byte_end), SUFFIX.to_owned());

            // Keep the cursor inside the original portion of the identifier. It
            // remains inside the identifier after `_wc` is appended.
            let offset = diag_span
                .byte_end
                .checked_sub(1)
                .context("diagnostic span was empty")?;
            offsets.push(TextSize::new(offset));
        }
        let edit = edit.finish();

        // Diagnostic offsets refer to the original text. Translate them through
        // all insertions before passing them to rust-analyzer.
        for offset in &mut offsets {
            *offset = edit
                .apply_to_offset(*offset)
                .context("replacement overlapped its cursor position")?;
        }
        edit.apply(&mut contents);
        write(path, contents)?;
        file_offset_map.insert(file_name, offsets);
    }

    Ok(file_offset_map)
}

fn remove_unused_imports() -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(["fix", "--allow-dirty", "--allow-no-vcs"]);
    let status = command.status()?;
    ensure!(status.success(), "command failed: {command:?}");
    Ok(())
}

fn fmt() -> Result<()> {
    let mut command = Command::new("cargo");
    command.arg("fmt");
    let status = command.status()?;
    ensure!(status.success(), "command failed: {command:?}");
    Ok(())
}
