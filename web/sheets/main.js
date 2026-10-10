import init, { SheetEditor } from "./pkg/quill_sheets_wasm.js";

const $ = (id) => document.getElementById(id);
const scroller = $("scroller");
const wrap = $("wrap");
const canvas = $("grid");
const ctx = canvas.getContext("2d");
const cellInput = $("cell");
const fxInput = $("fx");
const nameInput = $("name");

const HEAD_W = 52;
const HEAD_H = 26;
const PAD = 5;
const MAX_ROWS = 1048576;
const MAX_COLS = 16384;
const GRID = "#e1e4e8";
const HEAD_BG = "#f3f4f6";
const HEAD_SEL = "#dbe5f7";
const HEAD_FG = "#4b5563";
const INK = "#1f2328";
const ACCENT = "#2563eb";
const SHADE = "rgba(37, 99, 235, 0.12)";
const font = (bold, italic) => `${italic ? "italic " : ""}${bold ? "bold " : ""}13px system-ui, -apple-system, "Segoe UI", sans-serif`;
const hex = (n) => `#${n.toString(16).padStart(6, "0")}`;

await init();
const ed = new SheetEditor();
ed.load_csv(
  "Item,Qty,Price,Total\nNotebook,12,3.5,=B2*C2\nPen,40,0.8,=B3*C3\nStapler,3,12.25,=B4*C4\nMarker set,8,6.4,=B5*C5\n,,Subtotal,=SUM(D2:D5)\n,,Tax (8%),=D6*8%\n,,Total,=D6+D7\n\nTip,Click a cell and type. Formulas start with =  e.g. =AVERAGE(B2:B5)"
);
ed.style(0, 0, 0, 3, "bold", "1");
ed.style(0, 0, 0, 3, "fill", "#e8eefb");
ed.style(1, 2, 7, 3, "numfmt", "currency");
ed.style(7, 2, 7, 3, "bold", "1");
ed.set_col_width(0, 130);
ed.set_col_width(2, 110);
ed.clear_history(); // the demo is the starting point, not something to undo

// ---- geometry ------------------------------------------------------------

let defCol = 100;
let defRow = 24;
let colOver = new Map();
let rowOver = new Map();
let nRows = 200;
let nCols = 26;
let colX = new Float64Array(2);
let rowY = new Float64Array(2);

function rebuildGeometry() {
  const s = ed.sizes();
  defCol = s.default_col;
  defRow = s.default_row;
  colOver = new Map(s.cols);
  rowOver = new Map(s.rows);
  const [usedRows, usedCols] = ed.used_range();
  nRows = Math.min(MAX_ROWS, Math.max(nRows, usedRows + 60));
  nCols = Math.min(MAX_COLS, Math.max(nCols, usedCols + 10));
  colX = new Float64Array(nCols + 1);
  for (let i = 0; i < nCols; i++) colX[i + 1] = colX[i] + (colOver.get(i) ?? defCol);
  rowY = new Float64Array(nRows + 1);
  for (let i = 0; i < nRows; i++) rowY[i + 1] = rowY[i] + (rowOver.get(i) ?? defRow);
  wrap.style.width = `${HEAD_W + colX[nCols]}px`;
  wrap.style.height = `${HEAD_H + rowY[nRows]}px`;
}

const colLeft = (c) => (c <= nCols ? colX[c] : colX[nCols] + (c - nCols) * defCol);
const rowTop = (r) => (r <= nRows ? rowY[r] : rowY[nRows] + (r - nRows) * defRow);
const colW = (c) => colOver.get(c) ?? defCol;
const rowH = (r) => rowOver.get(r) ?? defRow;

/** Largest index i in [0, n-1] with arr[i] <= v. */
function indexAt(arr, n, v) {
  let lo = 0;
  let hi = n - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (arr[mid] <= v) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

function colName(i) {
  let s = "";
  for (let n = i + 1; n > 0; n = Math.floor((n - 1) / 26)) s = String.fromCharCode(65 + ((n - 1) % 26)) + s;
  return s;
}

// ---- selection -----------------------------------------------------------

/** Anchor (ar, ac) is the active cell; (fr, fc) is the moving corner. */
let sel = { ar: 0, ac: 0, fr: 0, fc: 0 };
const range = () => [Math.min(sel.ar, sel.fr), Math.min(sel.ac, sel.fc), Math.max(sel.ar, sel.fr), Math.max(sel.ac, sel.fc)];
const isWholeCols = () => range()[0] === 0 && range()[2] === MAX_ROWS - 1;
const isWholeRows = () => range()[1] === 0 && range()[3] === MAX_COLS - 1;

/** The selection limited to where data exists (for copy, fill and stats). */
function usedRange([r1, c1, r2, c2]) {
  const [ur, uc] = ed.used_range();
  return [r1, c1, Math.max(r1, Math.min(r2, ur - 1)), Math.max(c1, Math.min(c2, uc - 1))];
}

function rangeLabel() {
  const [r1, c1, r2, c2] = range();
  if (isWholeCols()) return c1 === c2 ? `${colName(c1)}:${colName(c1)}` : `${colName(c1)}:${colName(c2)}`;
  if (isWholeRows()) return r1 === r2 ? `${r1 + 1}:${r1 + 1}` : `${r1 + 1}:${r2 + 1}`;
  return r1 === r2 && c1 === c2 ? ed.a1(r1, c1) : `${ed.a1(r1, c1)}:${ed.a1(r2, c2)}`;
}

// ---- drawing -------------------------------------------------------------

let cachedCells = [];

function draw() {
  const vw = scroller.clientWidth;
  const vh = scroller.clientHeight;
  if (vw === 0 || vh === 0) return;
  const dpr = window.devicePixelRatio || 1;
  if (canvas.width !== Math.round(vw * dpr) || canvas.height !== Math.round(vh * dpr)) {
    canvas.width = Math.round(vw * dpr);
    canvas.height = Math.round(vh * dpr);
    canvas.style.width = `${vw}px`;
    canvas.style.height = `${vh}px`;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const sl = scroller.scrollLeft;
  const st = scroller.scrollTop;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, vw, vh);

  const gx = HEAD_W - sl;
  const gy = HEAD_H - st;
  const c1 = indexAt(colX, nCols, sl);
  const c2 = indexAt(colX, nCols, sl + vw - HEAD_W);
  const r1 = indexAt(rowY, nRows, st);
  const r2 = indexAt(rowY, nRows, st + vh - HEAD_H);
  const X = (c) => gx + colLeft(c);
  const Y = (r) => gy + rowTop(r);
  const cells = ed.visible(r1, r2, c1, c2);
  cachedCells = cells;
  const occupied = new Set(cells.filter((c) => c.kind !== "empty").map((c) => c.r * MAX_COLS + c.c));
  const [sr1, sc1, sr2, sc2] = range();

  // Grid area.
  ctx.save();
  ctx.beginPath();
  ctx.rect(HEAD_W, HEAD_H, vw - HEAD_W, vh - HEAD_H);
  ctx.clip();

  for (const cell of cells) {
    if (cell.fill == null) continue;
    ctx.fillStyle = hex(cell.fill);
    ctx.fillRect(X(cell.c), Y(cell.r), colW(cell.c), rowH(cell.r));
  }

  // Selection shading (the active cell stays unshaded).
  const sx = X(sc1);
  const sy = Y(sr1);
  const sw = X(Math.min(sc2, MAX_COLS - 1) + 1) - sx;
  const sh = Y(Math.min(sr2, MAX_ROWS - 1) + 1) - sy;
  if (sr1 !== sr2 || sc1 !== sc2) {
    ctx.fillStyle = SHADE;
    ctx.fillRect(sx, sy, sw, sh);
    const ac = cells.find((c) => c.r === sel.ar && c.c === sel.ac);
    ctx.fillStyle = ac?.fill != null ? hex(ac.fill) : "#fff";
    ctx.fillRect(X(sel.ac), Y(sel.ar), colW(sel.ac), rowH(sel.ar));
  }

  // Grid lines.
  ctx.strokeStyle = GRID;
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (let c = c1; c <= c2 + 1; c++) {
    const x = Math.round(X(c)) - 0.5;
    ctx.moveTo(x, HEAD_H);
    ctx.lineTo(x, vh);
  }
  for (let r = r1; r <= r2 + 1; r++) {
    const y = Math.round(Y(r)) - 0.5;
    ctx.moveTo(HEAD_W, y);
    ctx.lineTo(vw, y);
  }
  ctx.stroke();

  // Text.
  ctx.textBaseline = "middle";
  for (const cell of cells) {
    if (cell.kind === "empty" || cell.text === "") continue;
    const left = X(cell.c);
    const top = Y(cell.r);
    const w = colW(cell.c);
    const h = rowH(cell.r);
    ctx.font = font(cell.bold, cell.italic);
    ctx.fillStyle = cell.color != null ? hex(cell.color) : cell.kind === "error" ? "#b42318" : INK;
    let text = cell.text;
    let textW = ctx.measureText(text).width;
    let clipRight = left + w;
    if (textW > w - 2 * PAD) {
      if (cell.kind === "number") {
        text = "#".repeat(Math.max(1, Math.floor((w - 2 * PAD) / ctx.measureText("#").width)));
        textW = ctx.measureText(text).width;
      } else if (cell.align === "left") {
        // Left-aligned text spills into empty cells on its right.
        for (let c = cell.c + 1; c <= nCols && !occupied.has(cell.r * MAX_COLS + c) && clipRight - left < textW + 2 * PAD; c++) {
          clipRight += colW(c);
        }
      }
    }
    ctx.save();
    ctx.beginPath();
    ctx.rect(left, top, clipRight - left, h);
    ctx.clip();
    let x = left + PAD;
    if (cell.align === "right") x = left + w - PAD - textW;
    else if (cell.align === "center") x = left + (w - textW) / 2;
    ctx.fillText(text, x, top + h / 2 + 1);
    ctx.restore();
  }

  // Selection outline and the active cell.
  ctx.strokeStyle = ACCENT;
  ctx.lineWidth = 2;
  ctx.strokeRect(sx + 0.5, sy + 0.5, sw - 1, sh - 1);
  ctx.restore();

  // Headers.
  ctx.font = "12px system-ui, sans-serif";
  ctx.textBaseline = "middle";
  ctx.fillStyle = HEAD_BG;
  ctx.fillRect(HEAD_W, 0, vw - HEAD_W, HEAD_H);
  ctx.fillRect(0, HEAD_H, HEAD_W, vh - HEAD_H);
  ctx.save();
  ctx.beginPath();
  ctx.rect(HEAD_W, 0, vw - HEAD_W, HEAD_H);
  ctx.clip();
  for (let c = c1; c <= c2; c++) {
    const x = X(c);
    const w = colW(c);
    if (c >= sc1 && c <= sc2) {
      ctx.fillStyle = HEAD_SEL;
      ctx.fillRect(x, 0, w, HEAD_H);
    }
    ctx.fillStyle = HEAD_FG;
    ctx.textAlign = "center";
    ctx.fillText(colName(c), x + w / 2, HEAD_H / 2 + 1);
  }
  ctx.restore();
  ctx.save();
  ctx.beginPath();
  ctx.rect(0, HEAD_H, HEAD_W, vh - HEAD_H);
  ctx.clip();
  for (let r = r1; r <= r2; r++) {
    const y = Y(r);
    const h = rowH(r);
    if (r >= sr1 && r <= sr2) {
      ctx.fillStyle = HEAD_SEL;
      ctx.fillRect(0, y, HEAD_W, h);
    }
    ctx.fillStyle = HEAD_FG;
    ctx.textAlign = "center";
    ctx.fillText(String(r + 1), HEAD_W / 2, y + h / 2 + 1);
  }
  ctx.restore();
  ctx.textAlign = "left";
  ctx.strokeStyle = "#c8ccd2";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, HEAD_H - 0.5);
  ctx.lineTo(vw, HEAD_H - 0.5);
  ctx.moveTo(HEAD_W - 0.5, 0);
  ctx.lineTo(HEAD_W - 0.5, vh);
  ctx.stroke();
}

// ---- editing -------------------------------------------------------------

/** null, or "enter" (typing replaces; arrows commit) / "edit" (arrows move the caret). */
let editing = null;

function placeEditor() {
  const { ar, ac } = sel;
  cellInput.style.left = `${HEAD_W + colLeft(ac)}px`;
  cellInput.style.top = `${HEAD_H + rowTop(ar)}px`;
  cellInput.style.height = `${rowH(ar)}px`;
  let width = colW(ac);
  if (editing) {
    ctx.font = font(false, false);
    width = Math.max(width, ctx.measureText(cellInput.value).width + 28);
  }
  cellInput.style.width = `${width}px`;
}

function startEdit(mode, text) {
  editing = mode;
  cellInput.value = text ?? ed.raw(sel.ar, sel.ac);
  cellInput.classList.add("editing");
  placeEditor();
  cellInput.focus({ preventScroll: true });
  const end = cellInput.value.length;
  cellInput.setSelectionRange(end, end);
}

function leaveEdit() {
  editing = null;
  cellInput.value = "";
  cellInput.classList.remove("editing");
  placeEditor();
  cellInput.focus({ preventScroll: true });
}

function commit(dr = 0, dc = 0) {
  if (!editing) return;
  ed.set_input(sel.ar, sel.ac, cellInput.value);
  leaveEdit();
  edited();
  if (dr || dc) move(dr, dc, {});
}

function cancelEdit() {
  if (editing) leaveEdit();
  syncBars();
}

cellInput.addEventListener("input", () => {
  if (!editing) {
    editing = "enter";
    cellInput.classList.add("editing");
  }
  fxInput.value = cellInput.value;
  placeEditor();
});

// ---- navigation ----------------------------------------------------------

function ensureVisible(r, c) {
  const vw = scroller.clientWidth;
  const vh = scroller.clientHeight;
  let sl = scroller.scrollLeft;
  let st = scroller.scrollTop;
  if (colLeft(c) < sl) sl = colLeft(c);
  else if (colLeft(c + 1) > sl + vw - HEAD_W) sl = colLeft(c + 1) - (vw - HEAD_W);
  if (rowTop(r) < st) st = rowTop(r);
  else if (rowTop(r + 1) > st + vh - HEAD_H) st = rowTop(r + 1) - (vh - HEAD_H);
  scroller.scrollLeft = sl;
  scroller.scrollTop = st;
}

function growIfNeeded() {
  const vw = scroller.clientWidth;
  const vh = scroller.clientHeight;
  const lastCol = indexAt(colX, nCols, scroller.scrollLeft + vw);
  const lastRow = indexAt(rowY, nRows, scroller.scrollTop + vh);
  let grew = false;
  if (lastRow > nRows - 30 && nRows < MAX_ROWS) {
    nRows = Math.min(MAX_ROWS, nRows + 200);
    grew = true;
  }
  if (lastCol > nCols - 5 && nCols < MAX_COLS) {
    nCols = Math.min(MAX_COLS, nCols + 10);
    grew = true;
  }
  if (grew) rebuildGeometry();
}

function move(dr, dc, { extend = false, jump = false } = {}) {
  let r = extend ? sel.fr : sel.ar;
  let c = extend ? sel.fc : sel.ac;
  if (jump) [r, c] = ed.edge(r, c, Math.sign(dr), Math.sign(dc));
  else {
    r = Math.max(0, Math.min(MAX_ROWS - 1, r + dr));
    c = Math.max(0, Math.min(MAX_COLS - 1, c + dc));
  }
  if (extend) {
    sel.fr = r;
    sel.fc = c;
  } else sel = { ar: r, ac: c, fr: r, fc: c };
  while ((r >= nRows - 30 && nRows < MAX_ROWS) || (c >= nCols - 5 && nCols < MAX_COLS)) {
    if (r >= nRows - 30) nRows = Math.min(MAX_ROWS, r + 200);
    if (c >= nCols - 5) nCols = Math.min(MAX_COLS, c + 10);
    rebuildGeometry();
  }
  selectionChanged(r, c);
}

function selectionChanged(focusR = sel.fr, focusC = sel.fc) {
  ensureVisible(focusR, focusC);
  placeEditor();
  syncBars();
  announce();
  draw();
}

function setSelection(next, ensure = true) {
  sel = next;
  if (ensure) selectionChanged();
  else {
    placeEditor();
    syncBars();
    draw();
  }
}

// ---- toolbar / bars -----------------------------------------------------

let savedRevision = ed.revision();
let fileName = null;
const isDirty = () => ed.revision() !== savedRevision;

function syncBars() {
  const [r1, c1] = range();
  if (document.activeElement !== nameInput) nameInput.value = rangeLabel();
  if (document.activeElement !== fxInput && !editing) fxInput.value = ed.raw(sel.ar, sel.ac);
  const f = ed.cell_format(sel.ar, sel.ac);
  $("bold").setAttribute("aria-pressed", String(f.bold));
  $("italic").setAttribute("aria-pressed", String(f.italic));
  for (const b of document.querySelectorAll("[data-align]")) b.setAttribute("aria-pressed", String(f.align === b.dataset.align));
  $("numfmt").value = f.numfmt;
  $("undo").disabled = !ed.can_undo();
  $("redo").disabled = !ed.can_redo();
  const title = `${isDirty() ? "• " : ""}${fileName ?? "Untitled"} — Quill Sheets`;
  if (document.title !== title) document.title = title;
  updateStats(r1, c1);
}

function updateStats() {
  const [r1, c1, r2, c2] = usedRange(range());
  const multi = range()[0] !== range()[2] || range()[1] !== range()[3];
  const s = ed.stats(r1, c1, r2, c2);
  const fmt = (x) => String(Number(x.toPrecision(12)));
  $("stats").textContent =
    multi && s.count > 0
      ? `Count: ${s.count}${s.numbers ? `   Sum: ${fmt(s.sum)}   Average: ${fmt(s.sum / s.numbers)}   Min: ${fmt(s.min)}   Max: ${fmt(s.max)}` : ""}`
      : "";
}

function announce() {
  const [r, c] = [sel.ar, sel.ac];
  const raw = ed.raw(r, c);
  $("announce").textContent = `${ed.a1(r, c)}, ${ed.display(r, c) || "empty"}${raw.startsWith("=") ? `, formula ${raw}` : ""}`;
}

/** After any change to the sheet. */
function edited() {
  rebuildGeometry(); // sizes and the used area may have changed (undo, insert, paste, …)
  placeEditor();
  syncBars();
  draw();
}

function apply(prop, value) {
  if (editing) commit();
  const [r1, c1, r2, c2] = range();
  ed.style(r1, c1, r2, c2, prop, value);
  edited();
  cellInput.focus({ preventScroll: true });
}

const act = (fn) => () => {
  if (editing) commit();
  fn();
  edited();
  cellInput.focus({ preventScroll: true });
};

$("bold").onclick = () => apply("bold", ed.cell_format(sel.ar, sel.ac).bold ? "0" : "1");
$("italic").onclick = () => apply("italic", ed.cell_format(sel.ar, sel.ac).italic ? "0" : "1");
for (const b of document.querySelectorAll("[data-align]")) {
  b.onclick = () => apply("align", ed.cell_format(sel.ar, sel.ac).align === b.dataset.align ? "auto" : b.dataset.align);
}
$("color").onchange = () => apply("color", $("color").value);
$("fill").onchange = () => apply("fill", $("fill").value);
$("fillClear").onclick = () => apply("fill", "");
$("numfmt").onchange = () => apply("numfmt", $("numfmt").value);
$("decMore").onclick = act(() => ed.adjust_decimals(...range(), 1));
$("decLess").onclick = act(() => ed.adjust_decimals(...range(), -1));
$("undo").onclick = act(() => ed.undo());
$("redo").onclick = act(() => ed.redo());
$("fillDown").onclick = act(fillDown);
$("fillRight").onclick = act(fillRight);
$("insRow").onclick = act(() => { const [r1, , r2] = range(); ed.insert("row", r1, r2 - r1 + 1); rebuildGeometry(); });
$("insCol").onclick = act(() => { const [, c1, , c2] = range(); ed.insert("col", c1, c2 - c1 + 1); rebuildGeometry(); });
$("delRow").onclick = act(() => { const [r1, , r2] = range(); ed.delete("row", r1, r2 - r1 + 1); rebuildGeometry(); });
$("delCol").onclick = act(() => { const [, c1, , c2] = range(); ed.delete("col", c1, c2 - c1 + 1); rebuildGeometry(); });

function fillDown() {
  let [r1, c1, r2, c2] = usedRange(range());
  if (r1 === r2 && r1 > 0) r1 -= 1; // a single row copies the row above
  ed.fill_down(r1, c1, r2, c2);
}

function fillRight() {
  let [r1, c1, r2, c2] = usedRange(range());
  if (c1 === c2 && c1 > 0) c1 -= 1;
  ed.fill_right(r1, c1, r2, c2);
}

// Name box and formula bar.
nameInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    const r = ed.parse_range(nameInput.value);
    if (r.length) setSelection({ ar: r[0], ac: r[1], fr: r[2], fc: r[3] });
    cellInput.focus({ preventScroll: true });
  } else if (e.key === "Escape") {
    nameInput.value = rangeLabel();
    cellInput.focus({ preventScroll: true });
  }
});
nameInput.addEventListener("focus", () => nameInput.select());
nameInput.addEventListener("blur", () => { nameInput.value = rangeLabel(); });

fxInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    ed.set_input(sel.ar, sel.ac, fxInput.value);
    edited();
    cellInput.focus({ preventScroll: true });
    move(1, 0, {});
  } else if (e.key === "Escape") {
    fxInput.value = ed.raw(sel.ar, sel.ac);
    cellInput.focus({ preventScroll: true });
  }
});

// ---- keyboard -------------------------------------------------------------

function clearSelection() {
  const [r1, c1, r2, c2] = range();
  ed.clear_contents(r1, c1, r2, c2);
  edited();
}

cellInput.addEventListener("keydown", (e) => {
  if (e.isComposing || e.keyCode === 229) return;
  const mod = e.metaKey || e.ctrlKey;
  const key = e.key;

  if (editing) {
    if (key === "Enter") { e.preventDefault(); commit(e.shiftKey ? -1 : 1, 0); }
    else if (key === "Tab") { e.preventDefault(); commit(0, e.shiftKey ? -1 : 1); }
    else if (key === "Escape") { e.preventDefault(); cancelEdit(); }
    else if (key === "F2") { e.preventDefault(); editing = editing === "edit" ? "enter" : "edit"; }
    else if (editing === "enter" && key.startsWith("Arrow") && !mod) {
      e.preventDefault();
      const d = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] }[key];
      commit(...d);
    }
    return;
  }

  if (mod) {
    const k = key.toLowerCase();
    if (k === "a") { e.preventDefault(); const [ur, uc] = ed.used_range(); setSelection({ ar: 0, ac: 0, fr: Math.max(0, ur - 1), fc: Math.max(0, uc - 1) }); }
    else if (k === "b") { e.preventDefault(); $("bold").click(); }
    else if (k === "i") { e.preventDefault(); $("italic").click(); }
    else if (k === "z") { e.preventDefault(); (e.shiftKey ? $("redo") : $("undo")).click(); }
    else if (k === "y") { e.preventDefault(); $("redo").click(); }
    else if (k === "d") { e.preventDefault(); $("fillDown").click(); }
    else if (k === "r") { e.preventDefault(); $("fillRight").click(); }
    else if (k === "s") { e.preventDefault(); saveFlow(); }
    else if (k === "o") { e.preventDefault(); openFlow(); }
    else if (key === "Home") { e.preventDefault(); setSelection({ ar: 0, ac: 0, fr: 0, fc: 0 }); }
    else if (key === "End") { e.preventDefault(); const [ur, uc] = ed.used_range(); const r = Math.max(0, ur - 1), c = Math.max(0, uc - 1); setSelection({ ar: r, ac: c, fr: r, fc: c }); }
    else if (key.startsWith("Arrow")) {
      e.preventDefault();
      const d = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] }[key];
      move(...d, { extend: e.shiftKey, jump: true });
    }
    return; // copy / cut / paste arrive as clipboard events
  }

  const page = Math.max(1, Math.floor((scroller.clientHeight - HEAD_H) / defRow) - 1);
  switch (key) {
    case "ArrowUp": move(-1, 0, { extend: e.shiftKey }); break;
    case "ArrowDown": move(1, 0, { extend: e.shiftKey }); break;
    case "ArrowLeft": move(0, -1, { extend: e.shiftKey }); break;
    case "ArrowRight": move(0, 1, { extend: e.shiftKey }); break;
    case "Tab": move(0, e.shiftKey ? -1 : 1, {}); break;
    case "Enter": move(e.shiftKey ? -1 : 1, 0, {}); break;
    case "PageDown": move(page, 0, { extend: e.shiftKey }); break;
    case "PageUp": move(-page, 0, { extend: e.shiftKey }); break;
    case "Home": move(0, -MAX_COLS, { extend: e.shiftKey }); break;
    case "Delete":
    case "Backspace": clearSelection(); break;
    case "F2": startEdit("edit"); break;
    default: return; // printable keys fall through and start an edit via the input event
  }
  e.preventDefault();
});

// ---- clipboard -------------------------------------------------------------

let lastCopied = null;

cellInput.addEventListener("copy", (e) => {
  if (editing) return;
  const [r1, c1, r2, c2] = usedRange(range());
  ed.copy(r1, c1, r2, c2);
  lastCopied = ed.range_to_tsv(r1, c1, r2, c2);
  e.clipboardData.setData("text/plain", lastCopied);
  e.preventDefault();
});

cellInput.addEventListener("cut", (e) => {
  if (editing) return;
  const [r1, c1, r2, c2] = usedRange(range());
  ed.copy(r1, c1, r2, c2);
  lastCopied = ed.range_to_tsv(r1, c1, r2, c2);
  e.clipboardData.setData("text/plain", lastCopied);
  e.preventDefault();
  ed.clear_contents(...range());
  edited();
});

cellInput.addEventListener("paste", (e) => {
  if (editing) return;
  e.preventDefault();
  const text = e.clipboardData.getData("text/plain");
  if (!text) return;
  const [r, c] = [Math.min(sel.ar, sel.fr), Math.min(sel.ac, sel.fc)];
  // Our own copy keeps formulas; text from elsewhere is typed in.
  const area = ed.has_clip() && text === lastCopied ? ed.paste(r, c) : ed.paste_tsv(r, c, text);
  edited();
  setSelection({ ar: area[0], ac: area[1], fr: area[2], fc: area[3] });
});

// ---- pointer ---------------------------------------------------------------

let drag = null;

function hit(e) {
  const rect = canvas.getBoundingClientRect();
  const x = e.clientX - rect.left;
  const y = e.clientY - rect.top;
  const sl = scroller.scrollLeft;
  const st = scroller.scrollTop;
  if (x < HEAD_W && y < HEAD_H) return { kind: "corner" };
  const gx = x - HEAD_W + sl;
  const gy = y - HEAD_H + st;
  const col = gx >= colX[nCols] ? nCols - 1 : indexAt(colX, nCols, Math.max(0, gx));
  const row = gy >= rowY[nRows] ? nRows - 1 : indexAt(rowY, nRows, Math.max(0, gy));
  if (y < HEAD_H) {
    // Near a column border: resize.
    for (const c of [col, col - 1]) {
      if (c >= 0 && Math.abs(gx - colLeft(c + 1)) <= 3) return { kind: "col-border", col: c, x };
    }
    return { kind: "col-head", col };
  }
  if (x < HEAD_W) {
    for (const r of [row, row - 1]) {
      if (r >= 0 && Math.abs(gy - rowTop(r + 1)) <= 3) return { kind: "row-border", row: r, y };
    }
    return { kind: "row-head", row };
  }
  return { kind: "cell", row, col };
}

canvas.addEventListener("pointermove", (e) => {
  if (drag) return;
  const h = hit(e);
  canvas.style.cursor = h.kind === "col-border" ? "col-resize" : h.kind === "row-border" ? "row-resize" : h.kind === "cell" ? "cell" : "default";
});

canvas.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  if (editing) commit();
  const h = hit(e);
  try {
    canvas.setPointerCapture(e.pointerId);
  } catch {
    // Not an active pointer (e.g. synthetic events); dragging still works while over the canvas.
  }
  if (h.kind === "col-border") {
    drag = { kind: "resize-col", col: h.col, startX: h.x, startW: colW(h.col) };
    return;
  }
  if (h.kind === "row-border") {
    drag = { kind: "resize-row", row: h.row, startY: h.y, startH: rowH(h.row) };
    return;
  }
  if (h.kind === "corner") {
    setSelection({ ar: 0, ac: 0, fr: MAX_ROWS - 1, fc: MAX_COLS - 1 });
  } else if (h.kind === "col-head") {
    const anchor = e.shiftKey ? sel.ac : h.col;
    setSelection({ ar: 0, ac: anchor, fr: MAX_ROWS - 1, fc: h.col });
    drag = { kind: "cols", anchor };
  } else if (h.kind === "row-head") {
    const anchor = e.shiftKey ? sel.ar : h.row;
    setSelection({ ar: anchor, ac: 0, fr: h.row, fc: MAX_COLS - 1 });
    drag = { kind: "rows", anchor };
  } else if (e.shiftKey) {
    setSelection({ ...sel, fr: h.row, fc: h.col });
    drag = { kind: "cells" };
  } else {
    setSelection({ ar: h.row, ac: h.col, fr: h.row, fc: h.col });
    drag = { kind: "cells" };
  }
  cellInput.focus({ preventScroll: true });
});

canvas.addEventListener("pointermove", (e) => {
  if (!drag) return;
  const rect = canvas.getBoundingClientRect();
  if (drag.kind === "resize-col") {
    ed.set_col_width(drag.col, drag.startW + (e.clientX - rect.left - drag.startX));
    rebuildGeometry();
    placeEditor();
    draw();
    return;
  }
  if (drag.kind === "resize-row") {
    ed.set_row_height(drag.row, drag.startH + (e.clientY - rect.top - drag.startY));
    rebuildGeometry();
    placeEditor();
    draw();
    return;
  }
  const h = hit(e);
  if (drag.kind === "cells" && h.kind === "cell") setSelection({ ...sel, fr: h.row, fc: h.col }, false);
  else if (drag.kind === "cols" && h.col !== undefined) setSelection({ ...sel, ac: drag.anchor, fc: h.col }, false);
  else if (drag.kind === "rows" && h.row !== undefined) setSelection({ ...sel, ar: drag.anchor, fr: h.row }, false);
});

function endDrag() {
  if (drag?.kind?.startsWith("resize")) edited();
  drag = null;
}
canvas.addEventListener("pointerup", endDrag);
canvas.addEventListener("pointercancel", endDrag);

canvas.addEventListener("dblclick", (e) => {
  const h = hit(e);
  if (h.kind === "cell") startEdit("edit");
  else if (h.kind === "col-border") autosizeColumn(h.col);
});

function autosizeColumn(c) {
  const [ur] = ed.used_range();
  let widest = 0;
  for (const cell of ed.visible(0, Math.max(0, ur - 1), c, c)) {
    ctx.font = font(cell.bold, cell.italic);
    widest = Math.max(widest, ctx.measureText(cell.text).width);
  }
  ed.set_col_width(c, Math.max(40, Math.ceil(widest) + 2 * PAD + 6));
  rebuildGeometry();
  placeEditor();
  edited();
}

scroller.addEventListener("scroll", () => {
  growIfNeeded();
  draw();
});
window.addEventListener("resize", () => { draw(); });
new ResizeObserver(() => draw()).observe(scroller);
scroller.addEventListener("mousedown", () => { if (!editing) cellInput.focus({ preventScroll: true }); });

// ---- files (CSV) -------------------------------------------------------------

function download(text, name, type) {
  const a = Object.assign(document.createElement("a"), { href: URL.createObjectURL(new Blob([text], { type })), download: name });
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

async function newFlow() {
  if (isDirty() && !window.confirm("You have unsaved changes. Discard them?")) return;
  ed.new_sheet();
  fileName = null;
  resetView();
}

function resetView() {
  nRows = 200;
  nCols = 26;
  rebuildGeometry();
  scroller.scrollTo(0, 0);
  sel = { ar: 0, ac: 0, fr: 0, fc: 0 };
  savedRevision = ed.revision();
  selectionChanged();
  cellInput.focus({ preventScroll: true });
}

async function openFlow() {
  if (isDirty() && !window.confirm("You have unsaved changes. Discard them?")) return;
  const input = Object.assign(document.createElement("input"), { type: "file", accept: ".csv,.tsv,.txt,text/csv" });
  input.onchange = async () => {
    const f = input.files[0];
    if (!f) return;
    try {
      ed.load_csv(await f.text());
    } catch (err) {
      alert(`Could not open ${f.name}: ${err?.message ?? err}`);
      return;
    }
    fileName = f.name;
    resetView();
  };
  input.click();
}

function saveFlow() {
  if (editing) commit();
  const base = (fileName ?? "spreadsheet").replace(/\.[a-z0-9]+$/i, "");
  download(ed.to_csv(), `${base}.csv`, "text/csv");
  fileName = `${base}.csv`;
  savedRevision = ed.revision();
  syncBars();
}

$("new").onclick = newFlow;
$("open").onclick = openFlow;
$("save").onclick = saveFlow;
window.addEventListener("beforeunload", (e) => {
  if (isDirty()) { e.preventDefault(); e.returnValue = ""; }
});

// ---- start ---------------------------------------------------------------------

rebuildGeometry();
savedRevision = ed.revision();
selectionChanged();
cellInput.focus({ preventScroll: true });
