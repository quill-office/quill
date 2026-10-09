import init, { Editor } from "./pkg/quill_wasm.js";

const PAGE = { w: 816, h: 1056, margin: 72 };
const FONT_FAMILY = "Georgia, 'Times New Roman', serif";
const TEXT_COLOR = "#1f2328";
const SELECTION_COLOR = "rgba(37, 99, 235, 0.3)";
const CARET_COLOR = "#2563eb";

const $ = (id) => document.getElementById(id);
const pagesEl = $("pages");
const ime = $("ime");
const statusEl = $("status");
const mctx = document.createElement("canvas").getContext("2d");

const fontFor = (bold, italic, size) => `${italic ? "italic " : ""}${bold ? "bold " : ""}${size}px ${FONT_FAMILY}`;
function measure(text, bold, italic, size) {
  mctx.font = fontFor(bold, italic, size);
  return mctx.measureText(text).width;
}

await init();
const editor = new Editor();
editor.set_measure(measure);
editor.set_page(PAGE.w, PAGE.h, PAGE.margin);
editor.load_text(`# Welcome to Quill
Quill is an open-source office suite written in Rust and compiled to WebAssembly. This page is a live editor: click anywhere, type, drag to select, and try the toolbar.
## What works today
- Selection with mouse, Shift+arrows, double-click for a word, triple-click for a paragraph
- Undo and redo, cut, copy and paste
- Bold, italic and underline; headings; bulleted and numbered lists
- Tables: use the Table button, then Tab to move between cells
- Pages, wrapping, and opening or saving .docx files
## Try it
1. Press Ctrl+Z after typing to undo a burst of typing at once
2. Use Up and Down to move between lines
3. Open a .docx, edit it, then save it again`);

let frame = null;
let blink = true;
let canvases = [];
const dpr = window.devicePixelRatio || 1;

function ensureCanvases(n) {
  while (canvases.length < n) {
    const c = document.createElement("canvas");
    c.width = PAGE.w * dpr; c.height = PAGE.h * dpr;
    c.style.width = PAGE.w + "px"; c.style.height = PAGE.h + "px";
    c.dataset.page = String(canvases.length);
    c.setAttribute("role", "presentation");
    pagesEl.appendChild(c);
    canvases.push(c);
  }
  while (canvases.length > n) canvases.pop().remove();
}

function paint() {
  if (!frame) return;
  const focused = document.activeElement === ime;
  canvases.forEach((canvas, p) => {
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, PAGE.w, PAGE.h);
    ctx.strokeStyle = "#9aa0a6";
    ctx.lineWidth = 1;
    for (const c of frame.layout.cells) if (c.page === p) ctx.strokeRect(c.x + 0.5, c.y + 0.5, c.w, c.h);
    ctx.fillStyle = SELECTION_COLOR;
    for (const r of frame.selection) if (r.page === p) ctx.fillRect(r.x, r.y, r.w, r.h);
    ctx.fillStyle = TEXT_COLOR;
    ctx.textBaseline = "alphabetic";
    for (const line of frame.layout.lines) {
      if (line.page !== p) continue;
      const by = line.y + line.baseline;
      if (line.marker && line.items[0]) {
        const it = line.items[0];
        ctx.font = fontFor(it.bold, false, it.size);
        ctx.fillText(line.marker.text, line.marker.x, by);
      } else if (line.marker) {
        ctx.font = fontFor(false, false, 17);
        ctx.fillText(line.marker.text, line.marker.x, by);
      }
      for (const it of line.items) {
        ctx.font = fontFor(it.bold, it.italic, it.size);
        ctx.fillText(it.text, it.x, by);
        if (it.underline) {
          const w = measure(it.text.trimEnd(), it.bold, it.italic, it.size);
          ctx.fillRect(it.x, by + 2, w, 1);
        }
      }
    }
    const c = frame.caret;
    if (blink && focused && c.page === p && !frame.state.has_selection) {
      ctx.fillStyle = CARET_COLOR;
      ctx.fillRect(c.x, c.y + 1, 1.5, c.h - 2);
    }
  });
}

function updateToolbar() {
  const s = frame.state;
  for (const b of document.querySelectorAll("[data-attr]")) b.setAttribute("aria-pressed", String(s[b.dataset.attr]));
  for (const b of document.querySelectorAll("[data-kind]")) b.setAttribute("aria-pressed", String(s.kind === b.dataset.kind));
  const sel = $("kind");
  sel.value = ["normal", "h1", "h2", "h3"].includes(s.kind) ? s.kind : "normal";
  $("tableTools").hidden = !s.in_table;
  $("table").disabled = s.in_table;
  $("undo").disabled = !s.can_undo;
  $("redo").disabled = !s.can_redo;
  const words = editor.text().split(/\s+/).filter(Boolean).length;
  statusEl.textContent = `Page ${frame.caret.page + 1} of ${frame.layout.pages} · ${words} words`;
}

function placeIme() {
  const canvas = canvases[frame.caret.page];
  if (!canvas) return;
  const r = canvas.getBoundingClientRect();
  ime.style.left = `${r.left + frame.caret.x}px`;
  ime.style.top = `${r.top + frame.caret.y}px`;
}

function keepCaretVisible() {
  const canvas = canvases[frame.caret.page];
  if (!canvas) return;
  const top = canvas.getBoundingClientRect().top + frame.caret.y;
  const bar = document.querySelector("header").offsetHeight;
  if (top < bar + 16) window.scrollBy(0, top - bar - 16);
  else if (top + frame.caret.h > window.innerHeight - 16) window.scrollBy(0, top + frame.caret.h - window.innerHeight + 16);
}

/** Re-layout after a change. */
function refresh({ scroll = true } = {}) {
  frame = editor.frame();
  ensureCanvases(frame.layout.pages);
  blink = true;
  paint();
  updateToolbar();
  placeIme();
  if (scroll) keepCaretVisible();
}

setInterval(() => { blink = !blink; paint(); }, 530);

// ---- pointer --------------------------------------------------------------

/** Page-local coordinates for a client point, snapping to the nearest page. */
function locate(clientX, clientY) {
  let best = 0, bestDist = Infinity;
  canvases.forEach((c, i) => {
    const r = c.getBoundingClientRect();
    const d = clientY < r.top ? r.top - clientY : clientY > r.bottom ? clientY - r.bottom : 0;
    if (d < bestDist) { bestDist = d; best = i; }
  });
  const r = canvases[best].getBoundingClientRect();
  return { page: best, x: clientX - r.left, y: clientY - r.top };
}

let dragging = false;
pagesEl.addEventListener("mousedown", (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  const p = locate(e.clientX, e.clientY);
  const clicks = Math.min(e.detail || 1, 3);
  editor.click(p.page, p.x, p.y, e.shiftKey, clicks);
  dragging = clicks === 1;
  ime.focus();
  refresh({ scroll: false });
});
window.addEventListener("mousemove", (e) => {
  if (!dragging) return;
  const p = locate(e.clientX, e.clientY);
  editor.click(p.page, p.x, p.y, true, 1);
  refresh({ scroll: false });
});
window.addEventListener("mouseup", () => { dragging = false; });

// ---- keyboard / text input -----------------------------------------------

let composing = false;
ime.addEventListener("compositionstart", () => { composing = true; });
ime.addEventListener("compositionend", (e) => {
  composing = false;
  if (e.data) editor.insert_text(e.data);
  ime.value = "";
  refresh();
});
ime.addEventListener("input", (e) => {
  if (composing || e.isComposing) return;
  if (ime.value) editor.insert_text(ime.value);
  ime.value = "";
  refresh();
});

ime.addEventListener("keydown", (e) => {
  if (composing || e.isComposing || e.keyCode === 229) return;
  const mod = e.metaKey || e.ctrlKey;
  const key = e.key.toLowerCase();
  if (mod) {
    switch (key) {
      case "z": e.shiftKey ? editor.redo() : editor.undo(); break;
      case "y": editor.redo(); break;
      case "a": editor.select_all(); break;
      case "b": editor.toggle_attr("bold"); break;
      case "i": editor.toggle_attr("italic"); break;
      case "u": editor.toggle_attr("underline"); break;
      default: return; // let copy/cut/paste events through
    }
  } else {
    switch (e.key) {
      case "Enter": editor.enter(); break;
      case "Tab": editor.tab(e.shiftKey); break;
      case "Backspace": editor.backspace(); break;
      case "Delete": editor.delete_forward(); break;
      case "ArrowLeft": editor.move_left(e.shiftKey); break;
      case "ArrowRight": editor.move_right(e.shiftKey); break;
      case "ArrowUp": editor.move_vertical(-1, e.shiftKey); break;
      case "ArrowDown": editor.move_vertical(1, e.shiftKey); break;
      case "Home": editor.home(e.shiftKey); break;
      case "End": editor.end(e.shiftKey); break;
      default: return;
    }
  }
  e.preventDefault();
  refresh();
});

ime.addEventListener("copy", (e) => {
  const t = editor.selected_text();
  if (!t) return;
  e.clipboardData.setData("text/plain", t);
  e.preventDefault();
});
ime.addEventListener("cut", (e) => {
  const t = editor.cut();
  if (t) e.clipboardData.setData("text/plain", t);
  e.preventDefault();
  refresh();
});
ime.addEventListener("paste", (e) => {
  const t = e.clipboardData.getData("text/plain").replace(/\r\n?/g, "\n");
  e.preventDefault();
  if (t) { editor.insert_text(t); refresh(); }
});
ime.addEventListener("focus", () => refresh({ scroll: false }));
ime.addEventListener("blur", () => { blink = false; paint(); });

// ---- toolbar ----------------------------------------------------------------

// Keep focus (and the selection) in the editor when clicking toolbar buttons.
document.querySelector("header").addEventListener("pointerdown", (e) => {
  if (e.target.closest("button")) e.preventDefault();
});
const act = (fn) => () => { fn(); ime.focus(); refresh(); };
$("undo").onclick = act(() => editor.undo());
$("redo").onclick = act(() => editor.redo());
for (const b of document.querySelectorAll("[data-attr]")) b.onclick = act(() => editor.toggle_attr(b.dataset.attr));
for (const b of document.querySelectorAll("[data-kind]")) b.onclick = act(() => editor.toggle_kind(b.dataset.kind));
$("table").onclick = act(() => editor.insert_table(3, 3));
for (const b of document.querySelectorAll("[data-table]")) b.onclick = act(() => editor.table_op(b.dataset.table));
$("kind").onchange = act(() => editor.toggle_kind($("kind").value));

const file = $("file");
$("open").onclick = () => file.click();
file.onchange = async () => {
  const f = file.files[0];
  if (!f) return;
  try { editor.open_docx(new Uint8Array(await f.arrayBuffer())); }
  catch (err) { alert(`Could not open ${f.name}: ${err.message ?? err}`); }
  file.value = "";
  ime.focus();
  window.scrollTo(0, 0);
  refresh({ scroll: false });
};
$("save").onclick = () => {
  const blob = new Blob([editor.save_docx()], { type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document" });
  const a = Object.assign(document.createElement("a"), { href: URL.createObjectURL(blob), download: "document.docx" });
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
};

ime.focus();
refresh({ scroll: false });
