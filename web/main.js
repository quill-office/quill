import init, { Editor } from "./pkg/quill_wasm.js";
import { files } from "./files.js";

// The spreadsheet app is a separate page; the desktop shell does not host it yet.
if (!files.isNative) document.getElementById("sheetsLink").hidden = false;

const FONT_FAMILY = "Georgia, 'Times New Roman', serif";
// Office documents name fonts that browsers often lack; fall back by style.
const MONO = /^(courier|consolas|menlo|monaco|lucida console|cascadia|source code|fira (code|mono))/i;
const SERIF = /^(times|cambria|georgia|garamond|palatino|book antiqua|century|baskerville|bookman|constantia|minion|sitka)/i;
const generic = (name) => (MONO.test(name) ? "monospace" : SERIF.test(name) ? "serif" : "sans-serif");
const TEXT_COLOR = "#1f2328";
const SELECTION_COLOR = "rgba(37, 99, 235, 0.3)";
const FIND_COLOR = "rgba(250, 204, 21, 0.55)";
const CARET_COLOR = "#2563eb";

const $ = (id) => document.getElementById(id);
const pagesEl = $("pages");
const ime = $("ime");
const statusEl = $("status");
const mctx = document.createElement("canvas").getContext("2d");

const fontFor = (bold, italic, size, font) =>
  `${italic ? "italic " : ""}${bold ? "bold " : ""}${size}px ${font ? `"${font}", ${generic(font)}` : FONT_FAMILY}`;
const cssColor = (rgb) => (rgb == null ? TEXT_COLOR : `#${rgb.toString(16).padStart(6, "0")}`);
function measure(text, bold, italic, size, font) {
  mctx.font = fontFor(bold, italic, size, font);
  return mctx.measureText(text).width;
}

await init();
const editor = new Editor();
editor.set_measure(measure);
editor.load_text(`# Welcome to Quill
Quill is an open-source office suite written in Rust and compiled to WebAssembly. This page is a live editor: click anywhere, type, drag to select, and try the toolbar.
## What works today
- Selection with mouse, Shift+arrows, double-click for a word, triple-click for a paragraph
- Undo and redo, cut, copy and paste
- Bold, italic and underline; fonts, sizes and colours; alignment; headings; lists
- Tables: use the Table button, then Tab to move between cells
- Images: use the Image button, or paste or drop one onto the page
- Headers and footers: double-click the top or bottom margin, or use Page no. for page numbers
- Find and replace: Ctrl+F to search, Ctrl+H to replace
- Pages, wrapping, and opening or saving .docx files
## Try it
1. Press Ctrl+Z after typing to undo a burst of typing at once
2. Use Up and Down to move between lines
3. Open a .docx, edit it, then save it again`);

let frame = null;
let blink = true;
let canvases = [];
const dpr = window.devicePixelRatio || 1;

let pageSize = { w: 816, h: 1056 };

/** Keep one canvas per page, all sized to the document's page. */
function ensureCanvases(n, size) {
  const changed = size.w !== pageSize.w || size.h !== pageSize.h;
  pageSize = size;
  while (canvases.length < n) {
    const c = document.createElement("canvas");
    c.dataset.page = String(canvases.length);
    c.setAttribute("role", "presentation");
    c.width = 0; // sized below
    pagesEl.appendChild(c);
    canvases.push(c);
  }
  while (canvases.length > n) canvases.pop().remove();
  for (const c of canvases) {
    if (changed || !c.width) {
      c.width = Math.round(size.w * dpr); c.height = Math.round(size.h * dpr);
      c.style.width = size.w + "px"; c.style.height = size.h + "px";
    }
  }
}

// Decoded images, keyed by the pixel data's uid (stable across undo/redo).
const imageCache = new Map();
function cachedImage(it) {
  let entry = imageCache.get(it.image.uid);
  if (!entry) {
    const url = URL.createObjectURL(new Blob([editor.image_data(it.image.id)], { type: editor.image_mime(it.image.id) }));
    const img = new Image();
    img.onload = () => paint();
    img.src = url;
    entry = img;
    imageCache.set(it.image.uid, entry);
  }
  return entry;
}
function drawImage(ctx, it, baseline) {
  const { w, h } = it.image;
  const img = cachedImage(it);
  if (img.complete && img.naturalWidth) ctx.drawImage(img, it.x, baseline - h, w, h);
  else { ctx.fillStyle = "#e5e7eb"; ctx.fillRect(it.x, baseline - h, w, h); ctx.fillStyle = TEXT_COLOR; }
}

/** Dashed line and label marking the header/footer area being edited. */
function drawGuide(ctx) {
  const header = frame.state.story === "header";
  const y = header ? frame.page.top : frame.page.h - frame.page.bottom;
  ctx.save();
  ctx.strokeStyle = CARET_COLOR; ctx.fillStyle = CARET_COLOR; ctx.lineWidth = 1;
  ctx.setLineDash([5, 4]);
  ctx.beginPath(); ctx.moveTo(0, y + 0.5); ctx.lineTo(frame.page.w, y + 0.5); ctx.stroke();
  ctx.setLineDash([]);
  ctx.font = "11px system-ui, sans-serif"; ctx.textBaseline = header ? "top" : "bottom";
  ctx.fillText(header ? "Header" : "Footer", 8, header ? y + 3 : y - 3);
  ctx.restore();
}

function paint() {
  if (!frame) return;
  const focused = document.activeElement === ime;
  canvases.forEach((canvas, p) => {
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, pageSize.w, pageSize.h);
    ctx.strokeStyle = "#9aa0a6";
    ctx.lineWidth = 1;
    for (const c of frame.layout.cells) if (c.page === p) ctx.strokeRect(c.x + 0.5, c.y + 0.5, c.w, c.h);
    ctx.fillStyle = FIND_COLOR;
    for (const r of frame.find.rects) if (r.page === p) ctx.fillRect(r.x, r.y, r.w, r.h);
    ctx.fillStyle = TEXT_COLOR;
    ctx.textBaseline = "alphabetic";
    for (const line of frame.layout.lines) {
      if (line.page !== p) continue;
      // The story not being edited is shown faded, as in Word.
      ctx.globalAlpha = line.story === "Body" ? (frame.state.story === "body" ? 1 : 0.35) : (line.story.toLowerCase() === frame.state.story ? 1 : 0.5);
      const by = line.y + line.baseline;
      if (line.marker && line.items[0]) {
        const it = line.items[0];
        ctx.font = fontFor(it.bold, false, it.size);
        ctx.fillStyle = TEXT_COLOR;
        ctx.fillText(line.marker.text, line.marker.x, by);
      } else if (line.marker) {
        ctx.font = fontFor(false, false, 16);
        ctx.fillStyle = TEXT_COLOR;
        ctx.fillText(line.marker.text, line.marker.x, by);
      }
      for (const it of line.items) {
        if (it.image) { drawImage(ctx, it, by); continue; }
        ctx.font = fontFor(it.bold, it.italic, it.size, it.font);
        ctx.fillStyle = cssColor(it.color);
        ctx.fillText(it.text, it.x, by);
        if (it.underline) {
          const w = measure(it.text.trimEnd(), it.bold, it.italic, it.size, it.font);
          ctx.fillRect(it.x, by + 2, w, 1);
        }
      }
    }
    ctx.globalAlpha = 1;
    if (frame.state.story !== "body") drawGuide(ctx);
    ctx.fillStyle = SELECTION_COLOR;
    for (const r of frame.selection) if (r.page === p) ctx.fillRect(r.x, r.y, r.w, r.h);
    const c = frame.caret;
    if (blink && focused && c.page === p && !frame.state.has_selection) {
      ctx.fillStyle = CARET_COLOR;
      ctx.fillRect(c.x, c.y + 1, 1.5, c.h - 2);
    }
  });
}

function updateFindCount() {
  if ($("findbar").hidden) return;
  const { count, index } = frame.find;
  $("findCount").textContent = !$("findQ").value ? "" : count === 0 ? "No matches" : index == null ? `${count} found` : `${index + 1} of ${count}`;
}

function updateToolbar() {
  const s = frame.state;
  for (const b of document.querySelectorAll("[data-attr]")) b.setAttribute("aria-pressed", String(s[b.dataset.attr]));
  for (const b of document.querySelectorAll("[data-kind]")) b.setAttribute("aria-pressed", String(s.kind === b.dataset.kind));
  $("paper").value = s.paper === "custom" ? "" : s.paper;
  $("spacing").value = String(s.line_spacing);
  if (!$("spacing").value) $("spacing").value = "0";
  $("font").value = s.font ?? "";
  if (s.font && $("font").value !== s.font) $("font").add(new Option(s.font, s.font, false, true));
  const pt = String(s.size / 2);
  if (![...$("size").options].some((o) => o.value === pt)) {
    const opt = new Option(pt, pt);
    const next = [...$("size").options].find((o) => parseFloat(o.value) > s.size / 2);
    $("size").add(opt, next ?? null);
  }
  $("size").value = pt;
  $("color").value = cssColor(s.color).slice(0, 7);
  for (const b of document.querySelectorAll("[data-align]")) b.setAttribute("aria-pressed", String(s.align === b.dataset.align));
  const sel = $("kind");
  sel.value = ["normal", "h1", "h2", "h3"].includes(s.kind) ? s.kind : "normal";
  const inHf = s.story !== "body";
  updateFindCount();
  updateTitle();
  $("hfTools").hidden = !inHf;
  $("editHeader").setAttribute("aria-pressed", String(s.story === "header"));
  $("editFooter").setAttribute("aria-pressed", String(s.story === "footer"));
  $("image").disabled = inHf;
  $("imageTools").hidden = !s.image_selected;
  $("tableTools").hidden = !s.in_table;
  $("table").disabled = s.in_table || inHf;
  $("undo").disabled = !s.can_undo;
  $("redo").disabled = !s.can_redo;
  const words = editor.text().replace(/\uFFFC/g, " ").split(/\s+/).filter(Boolean).length;
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
  ensureCanvases(frame.layout.pages, frame.page);
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
      case "l": editor.set_align("left"); break;
      case "e": editor.set_align("center"); break;
      case "r": editor.set_align("right"); break;
      case "j": editor.set_align("justify"); break;
      default: return; // let copy/cut/paste events through
    }
  } else {
    switch (e.key) {
      case "Escape":
        if (frame.state.story === "body") return;
        editor.enter_story("body", frame.caret.page);
        break;
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
  const imageFile = [...(e.clipboardData.files ?? [])].find((f) => f.type.startsWith("image/"));
  if (imageFile) { e.preventDefault(); insertImageFile(imageFile); return; }
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
for (const pt of [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 32, 36, 48, 72]) $("size").add(new Option(String(pt), String(pt)));
$("font").onchange = act(() => editor.set_font($("font").value));
$("size").onchange = act(() => editor.set_size(Math.round(parseFloat($("size").value) * 2)));
$("color").onchange = act(() => editor.set_color(parseInt($("color").value.slice(1), 16)));
$("colorReset").onclick = act(() => editor.set_color(-1));
for (const b of document.querySelectorAll("[data-align]")) b.onclick = act(() => editor.set_align(b.dataset.align));
$("paper").onchange = act(() => { if ($("paper").value) editor.set_paper($("paper").value); });
$("spacing").onchange = act(() => editor.set_line_spacing(parseInt($("spacing").value, 10)));
$("indentMore").onclick = act(() => editor.indent(720));
$("indentLess").onclick = act(() => editor.indent(-720));
$("pageNumbers").onclick = act(() => editor.add_page_numbers());
const toggleStory = (name) => () => editor.enter_story(frame.state.story === name ? "body" : name, frame.caret.page);
$("editHeader").onclick = act(toggleStory("header"));
$("editFooter").onclick = act(toggleStory("footer"));
$("closeHf").onclick = act(() => editor.enter_story("body", frame.caret.page));
for (const b of document.querySelectorAll("[data-field]")) b.onclick = act(() => editor.insert_field(b.dataset.field));
$("table").onclick = act(() => editor.insert_table(3, 3));
for (const b of document.querySelectorAll("[data-table]")) b.onclick = act(() => editor.table_op(b.dataset.table));
$("kind").onchange = act(() => editor.toggle_kind($("kind").value));

// ---- images -----------------------------------------------------------------

const MAX_IMAGE_W = 600;
const IMAGE_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/bmp"];

async function insertImageFile(f) {
  if (!IMAGE_TYPES.includes(f.type)) { alert(`${f.name || "That file"} isn't a supported image (PNG, JPEG, GIF, WebP or BMP).`); return; }
  try {
    const bitmap = await createImageBitmap(f);
    const scale = Math.min(1, MAX_IMAGE_W / bitmap.width);
    const [w, h] = [Math.max(1, Math.round(bitmap.width * scale)), Math.max(1, Math.round(bitmap.height * scale))];
    bitmap.close();
    editor.insert_image(new Uint8Array(await f.arrayBuffer()), f.type, w, h);
  } catch (err) {
    alert(`Could not insert ${f.name || "image"}: ${err.message ?? err}`);
    return;
  }
  ime.focus();
  refresh();
}

$("image").onclick = () => $("imageFile").click();
$("imageFile").onchange = async () => {
  const f = $("imageFile").files[0];
  $("imageFile").value = "";
  if (f) await insertImageFile(f);
};
$("imgSmaller").onclick = act(() => editor.scale_images(0.8));
$("imgLarger").onclick = act(() => editor.scale_images(1.25));
pagesEl.addEventListener("dragover", (e) => { if ([...e.dataTransfer.types].includes("Files")) e.preventDefault(); });
pagesEl.addEventListener("drop", async (e) => {
  const f = [...e.dataTransfer.files].find((f) => f.type.startsWith("image/"));
  if (!f) return;
  e.preventDefault();
  const p = locate(e.clientX, e.clientY);
  editor.click(p.page, p.x, p.y, false, 1);
  await insertImageFile(f);
});

// ---- find and replace -----------------------------------------------------

const findbar = $("findbar");
function applySearch({ select = true } = {}) {
  editor.set_find($("findQ").value, $("findCase").checked, $("findWord").checked);
  if (select && $("findQ").value) editor.find_current();
  refresh();
}
function openFind(focusReplace = false) {
  const sel = editor.selected_text();
  if (sel && !sel.includes("\n")) $("findQ").value = sel;
  findbar.hidden = false;
  (focusReplace ? $("findR") : $("findQ")).focus();
  (focusReplace ? $("findR") : $("findQ")).select();
  applySearch();
}
function closeFind() {
  findbar.hidden = true;
  editor.set_find("", false, false);
  ime.focus();
  refresh();
}
function step(backwards) {
  applySearch({ select: false });
  editor.find_next(backwards);
  refresh();
}
$("find").onclick = () => openFind();
$("findClose").onclick = closeFind;
$("findNext").onclick = () => step(false);
$("findPrev").onclick = () => step(true);
$("replaceOne").onclick = () => {
  applySearch({ select: false });
  editor.replace_current($("findR").value);
  refresh();
};
$("replaceAll").onclick = () => {
  applySearch({ select: false });
  const n = editor.replace_all($("findR").value);
  refresh();
  $("findCount").textContent = n ? `Replaced ${n}` : "No matches";
};
for (const el of [$("findCase"), $("findWord")]) el.onchange = () => applySearch();
$("findQ").addEventListener("input", () => applySearch());
for (const el of [$("findQ"), $("findR")]) {
  el.addEventListener("keydown", (e) => {
    if (e.key === "Escape") { e.preventDefault(); closeFind(); }
    else if (e.key === "Enter") { e.preventDefault(); el === $("findR") ? $("replaceOne").click() : step(e.shiftKey); }
  });
}
document.addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
  const k = e.key.toLowerCase();
  if (k === "f" || k === "h") { e.preventDefault(); openFind(k === "h"); }
});

// ---- documents: new / open / save ------------------------------------------

let currentName = null;      // file name of the open document, if it has one
let savedRevision = 0;       // editor revision at the last save/open
const isDirty = () => editor.revision() !== savedRevision;
let shownTitle = "";
function updateTitle() {
  const title = `${isDirty() ? "• " : ""}${currentName ?? "Untitled"} — Quill Write`;
  if (title !== shownTitle) { shownTitle = title; files.setTitle(title).catch(() => {}); }
}
function markClean() { savedRevision = editor.revision(); updateTitle(); }
const afterLoad = () => { ime.focus(); window.scrollTo(0, 0); refresh({ scroll: false }); };

async function newFlow() {
  if (isDirty() && !(await files.confirmDiscard())) return;
  editor.new_document();
  currentName = null;
  await files.forget();
  markClean();
  afterLoad();
}

async function openFlow() {
  if (isDirty() && !(await files.confirmDiscard())) return;
  let picked;
  try { picked = await files.open(); }
  catch (err) { alert(`Could not open the file: ${err?.message ?? err}`); return; }
  if (!picked) return;
  try { editor.open_docx(picked.bytes); }
  catch (err) { alert(`Could not open ${picked.name}: ${err.message ?? err}`); return; }
  currentName = picked.name;
  markClean();
  afterLoad();
}

async function saveFlow(saveAs = false) {
  try {
    const name = await files.saveDocx(editor.save_docx(), { saveAs, name: currentName });
    if (name) { currentName = name; markClean(); }
  } catch (err) {
    alert(`Could not save: ${err?.message ?? err}`);
  }
}

$("new").onclick = newFlow;
$("open").onclick = openFlow;
$("save").onclick = () => saveFlow(false);
document.addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
  const k = e.key.toLowerCase();
  if (k === "s") { e.preventDefault(); saveFlow(e.shiftKey); }
  else if (k === "o") { e.preventDefault(); openFlow(); }
});
// Closing with unsaved changes asks first (natively in the app; the browser shows its own prompt).
files.onCloseRequest(Object.assign(async () => !isDirty() || (await files.confirmDiscard()), { sync: () => !isDirty() }));
const MAX_PDF_IMAGE = 1600; // px on the long side; keeps the PDF reasonable
async function decodeForPdf(id) {
  const blob = new Blob([editor.image_data(id)], { type: editor.image_mime(id) });
  const bitmap = await createImageBitmap(blob);
  const scale = Math.min(1, MAX_PDF_IMAGE / Math.max(bitmap.width, bitmap.height));
  const [w, h] = [Math.max(1, Math.round(bitmap.width * scale)), Math.max(1, Math.round(bitmap.height * scale))];
  const c = document.createElement("canvas");
  c.width = w; c.height = h;
  const g = c.getContext("2d");
  g.drawImage(bitmap, 0, 0, w, h);
  bitmap.close();
  return { w, h, rgba: new Uint8Array(g.getImageData(0, 0, w, h).data.buffer) };
}
$("pdf").onclick = async () => {
  $("pdf").disabled = true;
  try {
    for (const id of editor.pdf_image_ids()) {
      try { const img = await decodeForPdf(id); editor.set_pdf_image(id, img.w, img.h, img.rgba); }
      catch (err) { console.warn(`Skipping image ${id} in the PDF:`, err); }
    }
    await files.savePdf(editor.export_pdf(currentName ?? "document"), { name: currentName });
  } catch (err) {
    alert(`Could not create the PDF: ${err.message ?? err}`);
  } finally {
    $("pdf").disabled = false;
  }
};

ime.focus();
refresh({ scroll: false });
files.ready().catch(() => {});
