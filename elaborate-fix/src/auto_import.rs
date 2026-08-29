use anyhow::{Context, Result, bail};
use ra_ap_hir::{ChangeWithProcMacros, PrefixKind};
use ra_ap_ide::{
    AnalysisHost, AssistConfig, AssistKind, AssistResolveStrategy, DiagnosticsConfig,
    ExprFillDefaultMode, FileId, FileRange, TextEdit, TextRange, TextSize,
};
use ra_ap_ide_db::imports::insert_use::{ImportGranularity, InsertUseConfig};
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::{AbsPathBuf, Vfs, VfsPath};
use std::{
    collections::BTreeMap,
    fs::{read_to_string, write},
    path::{Path, PathBuf},
};

pub(crate) type FileOffsetMap = BTreeMap<String, Vec<TextSize>>;

/// Loads the fully rewritten workspace and applies rust-analyzer's auto-import assists.
/// Loading happens after every rewrite because auto-import is driven by the unresolved
/// names present in the source text, rather than by names passed to an API.
pub(crate) fn auto_import(workspace_root: &Path, file_offset_map: FileOffsetMap) -> Result<()> {
    if file_offset_map.is_empty() {
        return Ok(());
    }

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        // A matching proc-macro server is an additional runtime dependency. This
        // tool accepts reduced analysis in macro-heavy code instead.
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: false,
        num_worker_threads: 1,
        proc_macro_processes: 1,
    };
    let (database, vfs, _proc_macro_server) = load_workspace_at(
        workspace_root,
        &CargoConfig::default(),
        &load_config,
        &|message| eprintln!("rust-analyzer: {message}"),
    )?;
    let mut host = AnalysisHost::with_database(database);
    let assist_config = assist_config();

    // `assists_with_fixes` requires diagnostic settings, but only ordinary
    // assists are relevant here.
    let mut diagnostics_config = DiagnosticsConfig::test_sample();
    diagnostics_config.enabled = false;

    for (file_name, mut offsets) in file_offset_map {
        let path = workspace_root.join(&file_name);
        let file_id = file_id(&vfs, &path)?;
        let mut contents = read_to_string(&path)?;

        for index in 0..offsets.len() {
            let offset = offsets[index];
            let Some(edit) =
                find_auto_import_edit(&host, &assist_config, &diagnostics_config, file_id, offset)?
            else {
                eprintln!(
                    "{file_name}: no elaborate auto-import needed or found at byte {offset:?}"
                );
                continue;
            };

            // Inserting a `use` near the top of the file shifts later call sites.
            // Translate their cursors before applying the edit.
            for pending_offset in &mut offsets[index + 1..] {
                *pending_offset = edit
                    .apply_to_offset(*pending_offset)
                    .context("auto-import edit overlapped a pending replacement")?;
            }
            edit.apply(&mut contents);

            // Each subsequent assist must analyze the imports already applied;
            // otherwise rust-analyzer could return stale or duplicate edits.
            let mut change = ChangeWithProcMacros::default();
            change.change_file(file_id, Some(contents.clone()));
            host.apply_change(change);
        }

        write(path, contents)?;
    }

    Ok(())
}

fn assist_config() -> AssistConfig {
    AssistConfig {
        snippet_cap: None,
        allowed: Some(vec![AssistKind::QuickFix]),
        insert_use: InsertUseConfig {
            granularity: ImportGranularity::Crate,
            enforce_granularity: true,
            prefix_kind: PrefixKind::Plain,
            group: false,
            skip_glob_imports: true,
        },
        prefer_no_std: false,
        prefer_prelude: true,
        prefer_absolute: false,
        assist_emit_must_use: false,
        term_search_fuel: 400,
        code_action_grouping: true,
        expr_fill_default: ExprFillDefaultMode::Todo,
        prefer_self_ty: false,
        show_rename_conflicts: true,
    }
}

/// Converts an on-disk path to the identifier used by rust-analyzer's VFS.
fn file_id(vfs: &Vfs, path: &Path) -> Result<FileId> {
    let path = AbsPathBuf::assert_utf8(PathBuf::from(path));
    vfs.file_id(&VfsPath::from(path))
        .map(|(file_id, _excluded)| file_id)
        .context("rewritten file was not loaded by rust-analyzer")
}

/// Returns the text edit for the first relevant `auto_import` assist at `offset`.
fn find_auto_import_edit(
    host: &AnalysisHost,
    assist_config: &AssistConfig,
    diagnostics_config: &DiagnosticsConfig,
    file_id: FileId,
    offset: TextSize,
) -> Result<Option<TextEdit>> {
    let frange = FileRange {
        file_id,
        range: TextRange::empty(offset),
    };
    let assists = host.analysis().assists_with_fixes(
        assist_config,
        diagnostics_config,
        AssistResolveStrategy::All,
        frange,
    )?;

    // The auto-import handler is private, so the public API runs the whole assist
    // set. The source supplies the symbol name; the label restricts the result to
    // this project's wrapper crate.
    let Some(assist) = assists.into_iter().find(|assist| {
        assist.id.0 == "auto_import" && assist.label.to_string().starts_with("Import `elaborate::")
    }) else {
        return Ok(None);
    };
    let mut source_change = assist
        .source_change
        .context("resolved auto-import had no source change")?;

    // A normal auto-import should only edit the requested source file. Reject
    // unexpected side effects rather than silently applying them.
    if !source_change.file_system_edits.is_empty() {
        bail!("auto-import unexpectedly included file-system edits");
    }
    let Some((edit, _snippet_edit)) = source_change.source_file_edits.remove(&file_id) else {
        bail!("auto-import did not edit the requested file");
    };

    Ok(Some(edit))
}
