const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;
const appWindow = window.__TAURI__.window.getCurrentWindow();
const $ = (id) => document.getElementById(id);
const qa = (sel) => document.querySelectorAll(sel);

const RES = [
  ["Original size", 0], ["144p", 144], ["240p", 240], ["360p", 360], ["480p", 480],
  ["720p HD", 720], ["1080p Full HD", 1080], ["1440p 2K", 1440], ["2160p 4K", 2160],
];
const FPS = [15, 24, 30, 60, 120];
const COUNTDOWN = [0, 3, 5, 10];
const FKEYS = ["F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];
const KEY = "simplecam.settings.v3";

const DEF = {
  source: "screen", monitor: 0, window: "", regionX: 0, regionY: 0, regionW: 800, regionH: 600,
  res: 6, fps: 2, quality: "medium", codec: "h264", custom: false, mbps: 8, compat: true,
  audioMic: false, micDevice: "", audioSystem: false, cursor: true, border: false,
  timestamp: false, caption: "", overlayPos: "br", overlayScale: 1,
  hkRec: "F9", hkPause: "F10", folder: "", countdown: 0,
  minimize: false, onTop: false, openAfter: false,
};
const s = { ...DEF };
try { Object.assign(s, JSON.parse(localStorage.getItem(KEY) || "{}")); } catch {}

let monitors = [], windows = [];
let recording = false, paused = false, counting = null, lastFile = null, elapsed = 0;

const save = () => localStorage.setItem(KEY, JSON.stringify(s));
const opts = () => ({
  source: s.source, monitor: s.monitor, window: s.window,
  regionX: s.regionX, regionY: s.regionY, regionW: s.regionW, regionH: s.regionH,
  height: RES[s.res][1], fps: FPS[s.fps], quality: s.quality, codec: s.codec,
  customMbps: s.custom ? s.mbps : null, cursor: s.cursor, border: s.border, compat: s.compat,
  folder: s.folder, audioMic: s.audioMic, micDevice: s.micDevice, audioSystem: s.audioSystem,
  timestamp: s.timestamp, caption: s.caption, overlayPos: s.overlayPos, overlayScale: s.overlayScale,
});

function setStatus(text, kind = "") {
  const el = $("status");
  el.textContent = text;
  el.className = kind;
}
const p2 = (n) => String(n).padStart(2, "0");
const fmtTime = (t) => `${p2(Math.floor(t / 3600))}:${p2(Math.floor(t / 60) % 60)}:${p2(t % 60)}`;
const fmtSize = (b) => (b > 1048576 ? (b / 1048576).toFixed(1) + " MB" : Math.round(b / 1024) + " KB");

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

async function refreshSummary() {
  try {
    const o = await invoke("compute_output", { opts: opts() });
    const src = { screen: "Full screen", region: "Region", window: "Window" }[s.source];
    const extras = [s.audioMic && "mic", s.audioSystem && "system audio", (s.timestamp || s.caption) && "overlay"].filter(Boolean);
    $("summary").textContent =
      `${src} \u2192 ${o.width}\u00d7${o.height} @ ${FPS[s.fps]} fps, ${(o.bitrate / 1e6).toFixed(1)} Mbps, MP4` +
      (extras.length ? ` + ${extras.join(", ")}` : "");
  } catch {}
}

async function loadWindows() {
  windows = (await invoke("list_windows")).filter((w) => w.title !== "SimpleCam");
  if (!windows.some((w) => w.title === s.window)) s.window = windows[0]?.title ?? "";
  const sel = $("window");
  sel.innerHTML = "";
  windows.forEach((w) => {
    const o = document.createElement("option");
    o.value = w.title;
    o.textContent = (w.title.length > 40 ? w.title.slice(0, 39) + "\u2026" : w.title) + `  (${w.width}\u00d7${w.height})`;
    sel.appendChild(o);
  });
  sel.value = s.window;
}

function syncInputs() {
  qa("[data-key]").forEach((el) => {
    const k = el.dataset.key;
    if (el.type === "checkbox") el.checked = !!s[k];
    else el.value = s[k];
  });
  qa('input[name="source"]').forEach((r) => (r.checked = r.value === s.source));
}

function render() {
  const locked = recording || counting !== null;
  qa('input[name="source"]').forEach((r) => (r.disabled = locked));
  qa("[data-key],#browse,#refresh,#pick").forEach((el) => {
    if (!el.dataset.free) el.disabled = locked;
  });
  $("quality").disabled = locked || s.custom;
  $("f-mbps").hidden = !s.custom;
  $("mbps-val").textContent = `${Number(s.mbps).toFixed(1)} Mbps`;
  $("p-screen").hidden = s.source !== "screen";
  $("p-window").hidden = s.source !== "window";
  $("p-region").hidden = s.source !== "region";
  $("btn-rec").classList.toggle("on", recording && !paused);
  $("btn-rec").disabled = recording;
  $("btn-pause").disabled = !recording;
  $("btn-stop").disabled = !recording && counting === null;
  $("pause-label").textContent = paused ? "Resume" : "Pause";
  document.querySelector(".statusbar").classList.toggle("rec", recording);
  $("timer").textContent = fmtTime(elapsed);
  qa(".k-rec").forEach((e) => (e.textContent = s.hkRec));
  qa(".k-pause").forEach((e) => (e.textContent = s.hkPause));
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
    recording = true; paused = false; elapsed = 0;
    setStatus("Recording\u2026", "err");
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
    if (left <= 0) { cancelCount(); startNow(); } else showOverlay(left);
  }, 1000);
  render();
}

async function stop() {
  try {
    lastFile = await invoke("stop_recording");
    setStatus("Saved: " + lastFile, "ok");
    if (s.openAfter) invoke("play_file", { path: lastFile });
  } catch (e) {
    setStatus(String(e), "err");
  }
  recording = false; paused = false;
  if (s.minimize) { try { await appWindow.unminimize(); await appWindow.setFocus(); } catch {} }
  render();
}

async function togglePause() {
  if (!recording) return;
  try {
    await invoke(paused ? "resume_recording" : "pause_recording");
    paused = !paused;
    setStatus(paused ? "Paused" : "Recording\u2026", paused ? "" : "err");
  } catch (e) { setStatus(String(e), "err"); }
  render();
}

function toggleRecord() {
  if (recording) stop();
  else if (counting !== null) { cancelCount(); setStatus("Cancelled"); render(); }
  else begin();
}

async function poll() {
  if (!recording) return;
  try {
    const st = await invoke("recording_status");
    elapsed = st.elapsed;
    paused = st.paused;
    $("size").textContent = fmtSize(st.size);
    $("timer").textContent = fmtTime(elapsed);
  } catch {}
}

function openOptions(tab) {
  qa("#tabs button").forEach((b) => b.classList.toggle("on", b.dataset.tab === tab));
  qa("#modal section").forEach((p) => p.classList.toggle("on", p.dataset.pane === tab));
  $("modal").hidden = false;
}

const ACTIONS = {
  record: toggleRecord,
  pause: togglePause,
  exit: () => appWindow.close(),
  "open-folder": () => invoke("open_path", { path: s.folder }),
  "play-last": () => (lastFile ? invoke("play_file", { path: lastFile }) : setStatus("No recording yet")),
  "src-region": () => { setSource("region"); pickRegion(); },
  "src-window": () => setSource("window"),
  "src-screen": () => setSource("screen"),
  "refresh-windows": async () => { await loadWindows(); save(); refreshSummary(); },
  "opt-video": () => openOptions("video"),
  "opt-audio": () => openOptions("audio"),
  "opt-cursor": () => openOptions("cursor"),
  "opt-overlay": () => openOptions("overlay"),
  "opt-hotkeys": () => openOptions("hotkeys"),
  "opt-program": () => openOptions("program"),
  reset: () => { localStorage.removeItem(KEY); location.reload(); },
  about: () => setStatus("SimpleCam 0.5.0 \u2013 lightweight screen recorder (Rust + Tauri)"),
};

function setSource(v) {
  if (recording || counting !== null) return;
  s.source = v; save(); syncInputs(); render(); refreshSummary();
}
async function pickRegion() {
  try { await invoke("pick_region", { monitor: s.monitor }); } catch (e) { setStatus(String(e), "err"); }
}

async function applyHotkeys() {
  try { await invoke("set_hotkeys", { rec: s.hkRec, pause: s.hkPause }); }
  catch (e) { setStatus("Hotkey error: " + e, "err"); }
}

async function init() {
  if (!s.folder) s.folder = await invoke("default_folder");
  monitors = await invoke("list_monitors");
  if (s.monitor >= monitors.length) s.monitor = 0;
  const monItems = monitors.map((m) => [m.index, m.name]);
  fill($("monitor"), monItems, s.monitor);
  fill($("monitor2"), monItems, s.monitor);
  fill($("res"), RES.map((r, i) => [i, r[0]]), s.res);
  fill($("fps"), FPS.map((f, i) => [i, `${f} fps`]), s.fps);
  fill($("hk-rec"), FKEYS.map((k) => [k, k]), s.hkRec);
  fill($("hk-pause"), FKEYS.map((k) => [k, k]), s.hkPause);
  const mics = await invoke("list_audio_inputs");
  fill($("mic"), [["", "Default microphone"], ...mics.map((m) => [m, m])], s.micDevice);
  await loadWindows();
  syncInputs();

  qa("[data-key]").forEach((el) => {
    const k = el.dataset.key;
    el.addEventListener("input", () => {
      const t = el.dataset.type;
      s[k] = t === "bool" ? el.checked : t === "int" ? parseInt(el.value, 10) || 0 : t === "float" ? parseFloat(el.value) : el.value;
      save();
      if (k === "monitor") { $("monitor").value = s.monitor; $("monitor2").value = s.monitor; }
      if (k === "onTop") appWindow.setAlwaysOnTop(s.onTop);
      if (k === "hkRec" || k === "hkPause") applyHotkeys();
      render(); refreshSummary();
    });
  });
  qa('input[name="source"]').forEach((r) => r.addEventListener("change", () => setSource(r.value)));

  // menus
  qa(".menu").forEach((m) => m.querySelector(".mbtn").addEventListener("click", (e) => {
    e.stopPropagation();
    const was = m.classList.contains("open");
    qa(".menu").forEach((x) => x.classList.remove("open"));
    if (!was) m.classList.add("open");
  }));
  document.addEventListener("click", () => qa(".menu").forEach((x) => x.classList.remove("open")));
  qa(".drop button").forEach((b) => b.addEventListener("click", () => ACTIONS[b.dataset.act]?.()));

  qa("#tabs button").forEach((b) => b.addEventListener("click", () => openOptions(b.dataset.tab)));
  $("close-modal").addEventListener("click", () => ($("modal").hidden = true));
  $("btn-rec").addEventListener("click", toggleRecord);
  $("btn-pause").addEventListener("click", togglePause);
  $("btn-stop").addEventListener("click", toggleRecord);
  $("refresh").addEventListener("click", ACTIONS["refresh-windows"]);
  $("pick").addEventListener("click", pickRegion);
  $("browse").addEventListener("click", async () => {
    const dir = await open({ directory: true, defaultPath: s.folder });
    if (typeof dir === "string") { s.folder = dir; syncInputs(); save(); }
  });

  await listen("hotkey-record", toggleRecord);
  await listen("hotkey-pause", togglePause);
  await listen("region-selected", (e) => {
    const r = e.payload;
    Object.assign(s, { regionX: r.x, regionY: r.y, regionW: r.w, regionH: r.h, source: "region" });
    save(); syncInputs(); render(); refreshSummary();
    setStatus(`Region ${r.w}\u00d7${r.h} at ${r.x},${r.y}`);
  });

  applyHotkeys();
  if (s.onTop) appWindow.setAlwaysOnTop(true);
  setInterval(poll, 500);
  render();
  refreshSummary();
  setStatus(`Ready. ${s.hkRec} = record/stop, ${s.hkPause} = pause`);
}

init().catch((e) => setStatus("Startup error: " + e, "err"));
