//! Quill Write desktop shell.
//!
//! The web app does all the editing; this crate only gives it native file
//! dialogs. The page never sees a file path: the commands below show the
//! dialog, read or write the chosen file, and remember the current path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// Largest document we will read (guards against opening something huge by mistake).
const MAX_OPEN_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Default)]
struct DocState {
    /// The .docx currently open, if it has been saved or opened from disk.
    path: Mutex<Option<PathBuf>>,
    /// Bytes of the file just opened, handed to the page by `take_opened`.
    opened: Mutex<Vec<u8>>,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_string())
}

/// Make sure `path` ends in `.ext` (any case); otherwise append it.
fn with_extension(path: PathBuf, ext: &str) -> PathBuf {
    let has = path
        .extension()
        .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext));
    if has {
        return path;
    }
    let mut s = path.into_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// A sensible suggested file name: the current document's, or "document".
fn default_save_name(current: Option<&Path>, ext: &str) -> String {
    let stem = current
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "document".to_string());
    format!("{stem}.{ext}")
}

/// Write through a temporary file in the same directory, so a failed or
/// interrupted save never leaves a half-written document behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_file_name(format!(".{}.quill-tmp", file_name(path)));
    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let len = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > max {
        return Err(format!(
            "{} is too large to open ({} MB)",
            file_name(path),
            len / (1024 * 1024)
        ));
    }
    fs::read(path).map_err(|e| e.to_string())
}

/// Ask the user to pick a .docx; returns its name and keeps its bytes for `take_opened`.
#[tauri::command]
async fn open_document(
    app: AppHandle,
    state: State<'_, DocState>,
) -> Result<Option<String>, String> {
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("Word document", &["docx"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    let bytes = read_limited(&path, MAX_OPEN_BYTES)?;
    *state.opened.lock().map_err(|e| e.to_string())? = bytes;
    let name = file_name(&path);
    *state.path.lock().map_err(|e| e.to_string())? = Some(path);
    Ok(Some(name))
}

/// The bytes of the document `open_document` just picked (sent as a raw binary reply).
#[tauri::command]
fn take_opened(state: State<'_, DocState>) -> Result<Response, String> {
    let bytes = std::mem::take(&mut *state.opened.lock().map_err(|e| e.to_string())?);
    Ok(Response::new(bytes))
}

/// Save the request body. Headers: `x-kind` is `docx` or `pdf`; `x-save-as` is
/// `1` to always show the dialog. A plain docx save writes to the current file
/// when there is one. Returns the saved file's name, or null if cancelled.
#[tauri::command]
async fn save_document(
    app: AppHandle,
    state: State<'_, DocState>,
    request: Request<'_>,
) -> Result<Option<String>, String> {
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected the document bytes".to_string());
    };
    let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok());
    let ext = match header("x-kind") {
        Some("docx") => "docx",
        Some("pdf") => "pdf",
        other => return Err(format!("unknown file kind: {other:?}")),
    };
    let save_as = header("x-save-as") == Some("1");
    let current = state.path.lock().map_err(|e| e.to_string())?.clone();

    let existing = if ext == "docx" && !save_as {
        current.clone()
    } else {
        None
    };
    let path = match existing {
        Some(p) => p,
        None => {
            let label = if ext == "pdf" {
                "PDF document"
            } else {
                "Word document"
            };
            let Some(picked) = app
                .dialog()
                .file()
                .set_file_name(default_save_name(current.as_deref(), ext))
                .add_filter(label, &[ext])
                .blocking_save_file()
            else {
                return Ok(None);
            };
            with_extension(picked.into_path().map_err(|e| e.to_string())?, ext)
        }
    };
    write_atomic(&path, bytes).map_err(|e| format!("could not write {}: {e}", file_name(&path)))?;
    if ext == "docx" {
        *state.path.lock().map_err(|e| e.to_string())? = Some(path.clone());
    }
    Ok(Some(file_name(&path)))
}

/// The page started a new document: the next save must ask for a name.
#[tauri::command]
fn forget_document(state: State<'_, DocState>) -> Result<(), String> {
    *state.path.lock().map_err(|e| e.to_string())? = None;
    Ok(())
}

/// The page calls this once the editor has initialised (debug builds log it,
/// which makes it easy to see that the webview, WASM and IPC all work).
#[tauri::command]
fn app_ready(title_ok: bool, close_ok: bool) {
    #[cfg(debug_assertions)]
    {
        let word = |ok: bool| if ok { "ok" } else { "DENIED" };
        eprintln!(
            "quill: editor ready (window title: {}, close guard: {})",
            word(title_ok),
            word(close_ok)
        );
    }
    #[cfg(not(debug_assertions))]
    let _ = (title_ok, close_ok);
}

/// Ask whether unsaved changes may be thrown away.
#[tauri::command]
async fn confirm_discard(app: AppHandle) -> bool {
    app.dialog()
        .message("You have unsaved changes. Discard them?")
        .title("Quill Write")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Discard".into(),
            "Cancel".into(),
        ))
        .blocking_show()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(DocState::default())
        .invoke_handler(tauri::generate_handler![
            open_document,
            take_opened,
            save_document,
            forget_document,
            app_ready,
            confirm_discard
        ])
        .run(tauri::generate_context!())
        .expect("error while running Quill Write");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh empty directory under the system temp dir.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("quill-desktop-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extensions_are_enforced_without_clobbering_names() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(with_extension(p("/a/report"), "docx"), p("/a/report.docx"));
        assert_eq!(
            with_extension(p("/a/report.DOCX"), "docx"),
            p("/a/report.DOCX"),
            "case-insensitive"
        );
        assert_eq!(
            with_extension(p("/a/notes.txt"), "docx"),
            p("/a/notes.txt.docx"),
            "never mislabel other types"
        );
        assert_eq!(with_extension(p("/a/v1.2"), "pdf"), p("/a/v1.2.pdf"));
    }

    #[test]
    fn suggested_names_follow_the_current_document() {
        assert_eq!(default_save_name(None, "docx"), "document.docx");
        assert_eq!(
            default_save_name(Some(Path::new("/x/Report.docx")), "pdf"),
            "Report.pdf"
        );
        assert_eq!(
            default_save_name(Some(Path::new("/x/Report.final.docx")), "pdf"),
            "Report.final.pdf"
        );
    }

    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temp_files() {
        let dir = scratch("atomic");
        let path = dir.join("doc.docx");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second, longer").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second, longer");
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("doc.docx")],
            "no temp file left: {names:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_keeps_the_old_file_and_cleans_up() {
        let dir = scratch("fail");
        let path = dir.join("doc.docx");
        write_atomic(&path, b"keep me").unwrap();
        // Writing "into" a path whose parent is a file fails.
        let bad = path.join("child.docx");
        assert!(write_atomic(&bad, b"nope").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"keep me");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_enforces_the_size_limit_and_reports_missing_files() {
        let dir = scratch("read");
        let path = dir.join("big.docx");
        fs::write(&path, vec![0u8; 2048]).unwrap();
        assert_eq!(read_limited(&path, 4096).unwrap().len(), 2048);
        let err = read_limited(&path, 1024).unwrap_err();
        assert!(
            err.contains("too large") && err.contains("big.docx"),
            "{err}"
        );
        assert!(read_limited(&dir.join("missing.docx"), 4096).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
