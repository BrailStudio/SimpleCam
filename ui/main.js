const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;
const appWindow = window.__TAURI__.window.getCurrentWindow();
const $ = (id) => document.getElementById(id);
const qa = (sel) => document.querySelectorAll(sel);

const RES = [
  ["Original size", 0], ["144p", 144], ["240p", 240], ["360p", 360], ["480p", 480],
  ["720p (HD)", 720], ["1080p (Full HD)", 1080], ["1440p (2K)", 1440], ["2160p (4K)", 2160],
];
const FPS = [15, 24, 30, 60, 120];
const COUNTDOWN = [0, 3, 5, 10];
const PRESETS = {
  tiny: [3, 0, "low"], standard: [5, 2, "medium"], high: [6, 2, "high"],
  smooth: [6, 3, "medium"], ultra: [8, 2, "high"],
};

const DEF = {
  source: "screen", monitor: 0, window: "", regionX: 0, regionY: 0, regionW: 800, regionH: 600,
  cursor: true, border: false, folder: "", openAfter: false,
  limitOn: false, limitSecs: 60, limitMbOn: false, limitMb: 500, countdown: 0,
  minimize: false, onTop: false,
  res: 6, fps: 2, codec: "h264", quality: "medium", bitrate: 0,
  audioSystem: false, sysGain: 100, audioMic: false, micDevice: "", micGain: 100, audioKbps: 160,
  timestamp: false, caption: "", overlayPos: "br", overlayScale: 1,
  hkMonitor: "Ctrl+PrintScreen", hkWindow: "Ctrl+Alt+PrintScreen",
  hkRegion: "Ctrl+Shift+PrintScreen", hkPause: "Ctrl+Shift+P",
};
let s = { ...DEF };
let monitors = [], windows = [];
let recording = false, paused = false, counting = null, lastFile = null, elapsed = 0;
let startAfterRegion = false, saveTimer = null;

const save = () => {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => invoke("save_settings", { json: JSON.stringify(s) }).catch(() => {}), 300);
};
const opts = () => ({
  source: s.source, monitor: s.monitor, window: s.window,
  regionX: s.regionX, regionY: s.regionY, regionW: s.regionW, regionH: s.regionH,
  height: RES[s.res][1], fps: FPS[s.fps], quality: s.quality, codec: s.codec,
  bitrateKbps: s.bitrate || 0, cursor: s.cursor, border: s.border, compat: true, folder: s.folder,
  audioMic: s.audioMic, micDevice: s.micDevice, audioSystem: s.audioSystem,
  micGain: s.micGain, sysGain: s.sysGain, audioKbps: s.audioKbps,
  timestamp: s.timestamp, caption: s.caption, overlayPos: s.overlayPos, overlayScale: s.overlayScale,
  limitSecs: s.limitOn ? s.limitSecs : 0, limitMb: s.limitMbOn ? s.limitMb : 0,
});

function setStatus(text, kind = "") {
  $("status").innerHTML = "";
  const span = document.createElement("span");
  span.className = kind;
  span.textContent = text;
  $("status").appendChild(span);
}
const p2 = (n) => String(n).padStart(2, "0");
const fmtTime = (t) => `${p2(Math.floor(t / 3600))}:${p2(Math.floor(t / 60) % 60)}:${p2(t % 60)}`;
const fmtSize = (b) => (b > 1048576 ? (b / 1048576).toFixed(1) + " MB" : Math.round(b / 1024) + " KB");

function fill(sel, items, selected) {
  sel.innerHTML = "";
  items.forEach(([value, label]) => {
    const o = document.createElement("option");
    o.value = value; o.textContent = label; sel.appendChild(o);
  });
  sel.value = String(selected);
}

async function refreshSummary() {
  try {
    const o = await invoke("compute_output", { opts: opts() });
    $("summary").textContent = `Output: ${o.width}\u00d7${o.height} @ ${FPS[s.fps]} fps, ${Math.round(o.bitrate / 1000)} kbit/s, MP4`;
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
    o.textContent = (w.title.length > 34 ? w.title.slice(0, 33) + "\u2026" : w.title) + ` (${w.width}\u00d7${w.height})`;
    sel.appendChild(o);
  });
  sel.value = s.window;
}

function syncInputs() {
  qa("[data-key]").forEach((el) => {
    const k = el.dataset.key;
    if (el.type === "checkbox") el.checked = !!s[k];
    else if (el.id !== "window") el.value = s[k];
  });
  if (windows.length) $("window").value = s.window;
  qa('input[name="source"]').forEach((r) => (r.checked = r.value === s.source));
}

function render() {
  const locked = recording || counting !== null;
  qa("[data-key],#browse,#refresh,#pick,#preset,#defaults,[name=source]").forEach((el) => {
    if (!el.dataset.free) el.disabled = locked;
  });
  $("r-monitor").hidden = s.source === "window";
  $("r-window").hidden = s.source !== "window";
  $("r-region").hidden = s.source !== "region";
  $("limitSecs").disabled = locked || !s.limitOn;
  $("limitMb").disabled = locked || !s.limitMbOn;
  $("mic").disabled = locked || !s.audioMic;
  $("sysGainV").textContent = s.sysGain + "%";
  $("micGainV").textContent = s.micGain + "%";
  $("btn-rec").classList.toggle("on", recording && !paused);
  $("btn-rec").disabled = recording || counting !== null;
  $("btn-pause").disabled = !recording;
  $("btn-stop").disabled = !recording && counting === null;
  $("pause-label").textContent = paused ? "Resume" : "Pause";
  $("timer").classList.toggle("on", recording);
  $("timer").textContent = fmtTime(elapsed);
  const b = $("badge");
  b.className = "badge" + (recording ? (paused ? " paused" : " rec") : "");
  b.textContent = recording ? (paused ? "Paused" : "Recording") : counting !== null ? "Starting" : "Ready";
  appWindow.setTitle(recording ? `${paused ? "\u23f8" : "\u25cf"} ${fmtTime(elapsed)} \u2013 SimpleCam` : "SimpleCam").catch(() => {});
}

function showOverlay(n) { $("overlay-num").textContent = n; $("overlay").hidden = n === null; }
function cancelCount() { if (counting !== null) clearInterval(counting); counting = null; showOverlay(null); }

async function startNow() {
  try {
    lastFile = await invoke("start_recording", { opts: opts() });
    recording = true; paused = false; elapsed = 0;
    setStatus("Recording\u2026", "err");
    if (s.minimize) await appWindow.minimize();
  } catch (e) { setStatus(String(e), "err"); }
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

async function finished(path) {
  recording = false; paused = false; lastFile = path || lastFile;
  setStatus("Saved: " + lastFile, "ok");
  if (s.openAfter && lastFile) invoke("play_file", { path: lastFile });
  if (s.minimize) { try { await appWindow.unminimize(); await appWindow.setFocus(); } catch {} }
  render();
}

async function stop() {
  if (counting !== null) { cancelCount(); setStatus("Cancelled"); return render(); }
  try { await finished(await invoke("stop_recording")); }
  catch (e) { recording = false; setStatus(String(e), "err"); render(); }
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

// ---- capture entry points (buttons + global shortcuts) ----
async function go(mode, fromHotkey = false) {
  if (recording || counting !== null) return stop();
  if (mode === "pause") return;
  startAfterRegion = false;
  if (mode === "region") {
    startAfterRegion = true;
    s.source = "region"; syncInputs(); render();
    return pickRegion();
  }
  if (mode === "window") {
    if (fromHotkey) {
      const t = await invoke("foreground_window_title").catch(() => null);
      await loadWindows();
      if (t && windows.some((w) => w.title === t)) s.window = t;
    }
    if (!s.window) return setStatus("No window to record. Open one and press Refresh.", "err");
  }
  s.source = mode; syncInputs(); save(); render();
  begin();
}

async function pickRegion() {
  try { await invoke("pick_region", { monitor: s.monitor }); }
  catch (e) { startAfterRegion = false; setStatus(String(e), "err"); }
}

async function poll() {
  if (!recording) return;
  try {
    const st = await invoke("recording_status");
    if (!st.recording) return; // auto-stop event will finish the UI
    elapsed = st.elapsed; paused = st.paused;
    $("size").textContent = fmtSize(st.size);
    render();
  } catch {}
}

async function applyHotkeys() {
  try {
    await invoke("set_hotkeys", { monitor: s.hkMonitor, window: s.hkWindow, region: s.hkRegion, pause: s.hkPause });
    setStatus("Ready. " + s.hkMonitor + " = capture monitor");
  } catch (e) { setStatus("Shortcut problem: " + e, "err"); }
}

function applyPreset(name) {
  const p = PRESETS[name];
  if (!p) return;
  [s.res, s.fps, s.quality] = p; s.bitrate = 0;
  save(); syncInputs(); render(); refreshSummary();
}

async function init() {
  try { Object.assign(s, JSON.parse(await invoke("load_settings") || "{}")); } catch {}
  if (!s.folder) s.folder = await invoke("default_folder");
  monitors = await invoke("list_monitors");
  if (s.monitor >= monitors.length) s.monitor = 0;
  fill($("monitor"), monitors.map((m) => [m.index, m.name]), s.monitor);
  fill($("res"), RES.map((r, i) => [i, r[0]]), s.res);
  fill($("fps"), FPS.map((f, i) => [i, `${f} fps`]), s.fps);
  const mics = await invoke("list_audio_inputs").catch(() => []);
  fill($("mic"), [["", "Default microphone"], ...mics.map((m) => [m, m])], s.micDevice);
  await loadWindows();
  syncInputs();

  qa("[data-key]").forEach((el) => {
    const k = el.dataset.key;
    el.addEventListener(el.dataset.evt || "input", () => {
      const t = el.dataset.type;
      s[k] = t === "bool" ? el.checked : t === "int" ? parseInt(el.value, 10) || 0 : el.value;
      save();
      if (k === "onTop") appWindow.setAlwaysOnTop(s.onTop);
      if (k.startsWith("hk")) applyHotkeys();
      if (k === "res" || k === "fps" || k === "quality") $("preset").value = "";
      render(); refreshSummary();
    });
  });
  qa('input[name="source"]').forEach((r) => r.addEventListener("change", () => { s.source = r.value; save(); render(); refreshSummary(); }));
  qa("[data-go]").forEach((b) => b.addEventListener("click", () => (b.dataset.go === "pause" ? togglePause() : go(b.dataset.go))));
  $("preset").addEventListener("change", (e) => applyPreset(e.target.value));
  $("btn-rec").addEventListener("click", () => go(s.source));
  $("btn-pause").addEventListener("click", togglePause);
  $("btn-stop").addEventListener("click", stop);
  $("refresh").addEventListener("click", async () => { await loadWindows(); save(); refreshSummary(); });
  $("pick").addEventListener("click", () => { startAfterRegion = false; pickRegion(); });
  $("browse").addEventListener("click", async () => {
    const dir = await open({ directory: true, defaultPath: s.folder });
    if (typeof dir === "string") { s.folder = dir; syncInputs(); save(); }
  });
  $("defaults").addEventListener("click", () => {
    s = { ...DEF, folder: s.folder };
    save(); syncInputs(); render(); refreshSummary(); applyHotkeys(); setStatus("Defaults restored");
  });

  await listen("hotkey-monitor", () => go("screen", true));
  await listen("hotkey-window", () => go("window", true));
  await listen("hotkey-region", () => go("region", true));
  await listen("hotkey-pause", togglePause);
  await listen("auto-stopped", (e) => finished(e.payload));
  await listen("region-selected", (e) => {
    const r = e.payload;
    Object.assign(s, { regionX: r.x, regionY: r.y, regionW: r.w, regionH: r.h, source: "region" });
    save(); syncInputs(); render(); refreshSummary();
    setStatus(`Region ${r.w}\u00d7${r.h} at ${r.x},${r.y}`);
    if (startAfterRegion) { startAfterRegion = false; begin(); }
  });

  await applyHotkeys();
  if (s.onTop) appWindow.setAlwaysOnTop(true);
  setInterval(poll, 500);
  render(); refreshSummary();
}

init().catch((e) => setStatus("Startup error: " + e, "err"));
