const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;
const appWindow = window.__TAURI__.window.getCurrentWindow();
const $ = (id) => document.getElementById(id);

const RES = [
  ["144p", 144], ["240p", 240], ["360p", 360], ["480p", 480],
  ["720p (HD)", 720], ["1080p (Full HD)", 1080], ["1440p (2K)", 1440], ["2160p (4K)", 2160],
];
const FPS = [15, 24, 30, 60, 120];
const COUNTDOWN = [0, 3, 5, 10];
const KEY = "simplecam.settings.v1";

const s = {
  source: "screen", monitor: 0, window: "", res: 5, fps: 2, quality: "medium",
  codec: "h264", custom: false, mbps: 8, countdown: 0, folder: "",
  cursor: true, border: false, minimize: false, onTop: false,
};
try { Object.assign(s, JSON.parse(localStorage.getItem(KEY) || "{}")); } catch {}

let monitors = [], windows = [];
let recording = false, counting = null, startedAt = 0, lastFile = null;

const save = () => localStorage.setItem(KEY, JSON.stringify(s));
const opts = () => ({
  source: s.source, monitor: s.monitor, window: s.window,
  height: RES[s.res][1], fps: FPS[s.fps], quality: s.quality, codec: s.codec,
  customMbps: s.custom ? s.mbps : null, cursor: s.cursor, border: s.border, folder: s.folder,
});

function setStatus(text, kind = "") {
  const el = $("status");
  el.textContent = text;
  el.className = "status " + kind;
}

function fill(sel, items, selected) {
  sel.innerHTML = "";
  items.forEach(([value, label]) => {
    const o = document.createElement("option");
    o.value = value;
    o.textContent = label;
    sel.appendChild(o);
  });
  sel.value = String(selected);
}

async function refreshPreview() {
  try {
    const o = await invoke("compute_output", { opts: opts() });
    $("preview").textContent =
      `Output ${o.width}\u00d7${o.height} \u00b7 ${FPS[s.fps]} fps \u00b7 ${(o.bitrate / 1e6).toFixed(1)} Mbps \u00b7 MP4`;
  } catch {}
}

async function loadWindows() {
  windows = await invoke("list_windows");
  windows = windows.filter((w) => w.title !== "SimpleCam");
  if (!windows.some((w) => w.title === s.window)) s.window = windows[0]?.title ?? "";
  const sel = $("window");
  sel.innerHTML = "";
  windows.forEach((w) => {
    const o = document.createElement("option");
    o.value = w.title;
    o.textContent = (w.title.length > 46 ? w.title.slice(0, 45) + "\u2026" : w.title) + `  (${w.width}\u00d7${w.height})`;
    sel.appendChild(o);
  });
  sel.value = s.window;
}

function render() {
  const locked = recording || counting !== null;
  document.querySelectorAll("#seg button").forEach((b) => {
    b.classList.toggle("on", b.dataset.source === s.source);
    b.disabled = locked;
  });
  document.querySelectorAll("[data-key],#browse,#refresh").forEach((el) => {
    if (!el.dataset.free) el.disabled = locked;
  });
  $("row-monitor").hidden = s.source !== "screen";
  $("row-window").hidden = s.source !== "window";
  $("row-quality").hidden = false;
  $("quality").disabled = locked || s.custom;
  $("row-mbps").hidden = !s.custom;
  $("mbps-val").textContent = `${Number(s.mbps).toFixed(1)} Mbps`;

  const btn = $("main-btn");
  btn.textContent = recording ? "Stop recording" : counting !== null ? "Cancel" : "Start recording";
  btn.classList.toggle("rec", recording);
  $("open-folder").hidden = locked;
  $("play-last").hidden = locked || !lastFile;
  $("timer").hidden = !recording;
}

function showOverlay(n) {
  $("overlay-num").textContent = n;
  $("overlay").hidden = n === null;
}

function cancelCount() {
  if (counting !== null) clearInterval(counting);
  counting = null;
  showOverlay(null);
}

async function startNow() {
  try {
    lastFile = await invoke("start_recording", { opts: opts() });
    recording = true;
    startedAt = Date.now();
    setStatus("Recording\u2026", "rec");
    if (s.minimize) await appWindow.minimize();
  } catch (e) {
    setStatus(String(e), "err");
  }
  render();
}

function begin() {
  const n = COUNTDOWN[s.countdown];
  if (!n) return startNow();
  let left = n;
  showOverlay(left);
  counting = setInterval(() => {
    left -= 1;
    if (left <= 0) {
      cancelCount();
      startNow();
    } else {
      showOverlay(left);
    }
  }, 1000);
  setStatus("Get ready\u2026");
  render();
}

async function stop() {
  try {
    lastFile = await invoke("stop_recording");
    setStatus("Saved: " + lastFile, "ok");
  } catch (e) {
    setStatus(String(e), "err");
  }
  recording = false;
  if (s.minimize) {
    try { await appWindow.unminimize(); await appWindow.setFocus(); } catch {}
  }
  render();
}

function toggle() {
  if (recording) stop();
  else if (counting !== null) { cancelCount(); setStatus("Cancelled"); render(); }
  else begin();
}

function tick() {
  if (!recording) return;
  const t = Math.floor((Date.now() - startedAt) / 1000);
  const p = (n) => String(n).padStart(2, "0");
  $("timer").textContent = `${p(Math.floor(t / 3600))}:${p(Math.floor(t / 60) % 60)}:${p(t % 60)}`;
}

async function init() {
  if (!s.folder) s.folder = await invoke("default_folder");
  monitors = await invoke("list_monitors");
  if (s.monitor >= monitors.length) s.monitor = 0;

  fill($("monitor"), monitors.map((m) => [m.index, m.name]), s.monitor);
  fill($("res"), RES.map((r, i) => [i, r[0]]), s.res);
  fill($("fps"), FPS.map((f, i) => [i, `${f} fps`]), s.fps);
  await loadWindows();

  document.querySelectorAll("[data-key]").forEach((el) => {
    const k = el.dataset.key;
    if (el.type === "checkbox") el.checked = !!s[k];
    else if (el.id !== "monitor" && el.id !== "res" && el.id !== "fps" && el.id !== "window") el.value = s[k];
    el.addEventListener("input", () => {
      const t = el.dataset.type;
      s[k] = t === "bool" ? el.checked : t === "int" ? parseInt(el.value, 10) : t === "float" ? parseFloat(el.value) : el.value;
      save();
      if (k === "onTop") appWindow.setAlwaysOnTop(s.onTop);
      render();
      refreshPreview();
    });
  });

  document.querySelectorAll("#seg button").forEach((b) =>
    b.addEventListener("click", () => { s.source = b.dataset.source; save(); render(); refreshPreview(); })
  );
  $("refresh").addEventListener("click", async () => { await loadWindows(); save(); refreshPreview(); });
  $("browse").addEventListener("click", async () => {
    const dir = await open({ directory: true, defaultPath: s.folder });
    if (typeof dir === "string") { s.folder = dir; $("folder").value = dir; save(); }
  });
  $("main-btn").addEventListener("click", toggle);
  $("open-folder").addEventListener("click", () => invoke("open_path", { path: s.folder }));
  $("play-last").addEventListener("click", () => lastFile && invoke("open_path", { path: lastFile }));

  await listen("hotkey", toggle);
  if (s.onTop) appWindow.setAlwaysOnTop(true);
  setInterval(tick, 250);
  render();
  refreshPreview();
}

init().catch((e) => setStatus("Startup error: " + e, "err"));
