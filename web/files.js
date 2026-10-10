// File operations, in the browser (file input + downloads) or in the desktop
// app (native dialogs through Rust commands; the page never gets file paths).

const tauri = window.__TAURI__;
export const isNative = Boolean(tauri?.core?.invoke);
const invoke = (cmd, args, opts) => tauri.core.invoke(cmd, args, opts);

const DOCX = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

function download(bytes, type, name) {
  const a = Object.assign(document.createElement("a"), {
    href: URL.createObjectURL(new Blob([bytes], { type })),
    download: name,
  });
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

const withExt = (name, ext) => (name && /\.[a-z0-9]+$/i.test(name) ? name.replace(/\.[a-z0-9]+$/i, `.${ext}`) : `${name || "document"}.${ext}`);

// Whether the window-close listener registered (desktop app only).
let closeGuard = Promise.resolve(false);

export const files = {
  isNative,

  /** Ask before throwing away unsaved changes. */
  async confirmDiscard() {
    return isNative ? invoke("confirm_discard") : window.confirm("You have unsaved changes. Discard them?");
  },

  /** Let the user pick a .docx. Resolves `{ name, bytes }`, or null if cancelled. */
  async open() {
    if (isNative) {
      const name = await invoke("open_document");
      if (!name) return null;
      return { name, bytes: new Uint8Array(await invoke("take_opened")) };
    }
    return new Promise((resolve) => {
      const input = Object.assign(document.createElement("input"), { type: "file", accept: ".docx" });
      input.onchange = async () => {
        const f = input.files[0];
        resolve(f ? { name: f.name, bytes: new Uint8Array(await f.arrayBuffer()) } : null);
      };
      input.addEventListener("cancel", () => resolve(null));
      input.click();
    });
  },

  /** Save a .docx; resolves the saved file's name, or null if cancelled. */
  async saveDocx(bytes, { saveAs = false, name = null } = {}) {
    if (isNative) {
      return invoke("save_document", bytes, { headers: { "x-kind": "docx", "x-save-as": saveAs ? "1" : "0" } });
    }
    const file = withExt(name, "docx");
    download(bytes, DOCX, file);
    return file;
  },

  /** Save a PDF; resolves the saved file's name, or null if cancelled. */
  async savePdf(bytes, { name = null } = {}) {
    if (isNative) {
      return invoke("save_document", bytes, { headers: { "x-kind": "pdf", "x-save-as": "1" } });
    }
    const file = withExt(name, "pdf");
    download(bytes, "application/pdf", file);
    return file;
  },

  /** Forget the current file (after "New"). */
  async forget() {
    if (isNative) await invoke("forget_document");
  },

  /** Tell the desktop shell the editor is up (no-op in the browser). */
  async ready() {
    if (!isNative) return;
    // Also report whether the permissions we rely on were actually granted.
    const titleOk = await tauri.window.getCurrentWindow().setTitle("Quill Write").then(() => true, () => false);
    await invoke("app_ready", { titleOk, closeOk: await closeGuard });
  },

  async setTitle(title) {
    document.title = title;
    if (isNative) await tauri.window.getCurrentWindow().setTitle(title);
  },

  /** Run `canClose()` before the window closes; closing is cancelled if it resolves false. */
  onCloseRequest(canClose) {
    if (isNative) {
      closeGuard = tauri.window
        .getCurrentWindow()
        .onCloseRequested(async (event) => {
          if (!(await canClose())) event.preventDefault();
        })
        .then(() => true, () => false);
    } else {
      window.addEventListener("beforeunload", (e) => {
        // The browser shows its own generic prompt; we can only say whether to show it.
        if (canClose.sync && !canClose.sync()) { e.preventDefault(); e.returnValue = ""; }
      });
    }
  },
};
