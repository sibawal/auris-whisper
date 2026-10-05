// Auris Whisper — логика окна. Без сборщиков и фреймворков: Tauri отдаёт
// эту папку как есть, API берём из window.__TAURI__ (withGlobalTauri).
"use strict";

const T = window.__TAURI__;
const invoke = T.core.invoke;
const listen = T.event.listen;
const $ = (id) => document.getElementById(id);
const IS_MAC = navigator.userAgent.includes("Mac");
const IS_LINUX = /Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent);

const MEDIA_EXT = ["mp3", "m4a", "wav", "aiff", "aif", "caf", "aac", "flac", "ogg", "oga", "opus",
  "mp4", "m4v", "mov", "webm", "mkv", "mka", "wma", "amr", "3gp", "3g2",
  "avi", "wmv", "asf", "ts", "mts", "m2ts", "mpg", "mpeg", "flv", "ac3", "eac3"];

const S = {
  settings: null,
  catalog: [],
  installed: [],
  downloading: new Set(),
  dl: {},          // id → последнее событие загрузки
  dlErrors: {},    // id → текст ошибки
  system: null,
  modelsDir: "",
  busy: false,
  recording: false,
  lastSaved: null,
  status: () => t("ready"),
  audioSeconds: 0,
  queue: "",
  stats: null,
  lastOnGpu: null,
};

// ---------- Строки ----------

function lang() { return S.settings?.uiLang === "en" ? "en" : "ru"; }
function t(key, ...args) {
  const v = STRINGS[lang()][key] ?? STRINGS.ru[key] ?? key;
  return typeof v === "function" ? v(...args) : v;
}

function timecode(sec) {
  const total = Math.max(0, Math.floor(sec));
  const h = Math.floor(total / 3600), m = Math.floor((total % 3600) / 60), s = total % 60;
  const p = (n) => String(n).padStart(2, "0");
  return h > 0 ? `${p(h)}:${p(m)}:${p(s)}` : `${p(m)}:${p(s)}`;
}

// Длительность для строки состояния: «0,4 с» для коротких, иначе 01:23
function duration(sec) {
  if (sec < 60) {
    const v = sec < 10 ? sec.toFixed(1) : Math.round(sec).toString();
    return (lang() === "ru" ? v.replace(".", ",") : v) + (lang() === "ru" ? " с" : " s");
  }
  return timecode(sec);
}

function bytes(n) {
  if (n >= 1024 ** 3) return (n / 1024 ** 3).toFixed(n >= 10 * 1024 ** 3 ? 0 : 1) + " " + t("gb");
  return Math.round(n / 1024 ** 2) + " " + t("mb");
}

function baseName(path) { return path.split(/[\\/]/).pop(); }

function modelName(id) {
  return MODEL_TEXT[id]?.name ?? id;
}

// ---------- Отрисовка ----------

function applyStrings() {
  document.documentElement.lang = lang();
  document.querySelectorAll("[data-t]").forEach((el) => { el.textContent = t(el.dataset.t); });
  document.querySelectorAll("[data-t-title]").forEach((el) => {
    const v = el.dataset.tTitle === "appCpuHint" ? t("appCpuHint", S.system?.cores ?? 1) : t(el.dataset.tTitle);
    el.title = v;
  });
  document.querySelectorAll("[data-t-placeholder]").forEach((el) => { el.placeholder = t(el.dataset.tPlaceholder); });
  $("revealLabel").textContent = IS_MAC ? t("revealMac") : t("revealOther");

  const sel = $("language");
  const current = S.settings.language;
  sel.innerHTML = "";
  for (const [code, title] of [["auto", t("autoDetect")], ...LANGUAGES]) {
    const o = document.createElement("option");
    o.value = code; o.textContent = title;
    sel.appendChild(o);
  }
  sel.value = current;

  document.querySelectorAll("#uiLang button").forEach((b) => b.classList.toggle("on", b.dataset.lang === lang()));
  renderStatus();
  renderModelChip();
  renderEngineInfo();
  if ($("modelsDialog").open) renderModels();
  refreshMics();
}

function renderStatus() {
  $("status").textContent = S.status();
  $("queueInfo").textContent = S.queue;
}

function setStatus(fn) { S.status = fn; renderStatus(); }

function renderModelChip() {
  const chip = $("modelChip");
  const id = S.settings.model;
  const installed = id && S.installed.some((m) => m.id === id);
  $("modelChipText").textContent = installed ? modelName(id) : t("noModelChip");
  chip.classList.toggle("attention", !installed);
}

function renderEngineInfo() {
  const sys = S.system;
  if (!sys) return;
  const gpu = S.settings.useGpu && sys.gpuBackend !== "CPU" && S.lastOnGpu !== false ? sys.gpuBackend : "CPU";
  $("engineInfo").textContent = gpu === "CPU" ? "CPU" : `⚡ ${gpu}`;
  $("engineInfo").title = (gpu === "CPU" ? t("gpuNone") : sys.gpuDevices.join(", ")) + " — " + t("modelsTitle");
}

function renderBusy() {
  const busy = S.busy, rec = S.recording;
  $("progressRow").hidden = !busy;
  $("openBtn").disabled = busy;
  $("dictateBtn").disabled = busy;
  $("language").disabled = busy;
  $("dropIdle").hidden = rec;
  $("dropRec").hidden = !rec;
  $("drop").classList.toggle("recording", rec);
  const empty = $("transcript").value.length === 0;
  $("clearBtn").disabled = empty;
  $("copyBtn").disabled = empty;
  $("saveBtn").disabled = empty;
  $("revealBtn").hidden = !S.lastSaved;
}

function setProgress(value, indeterminate = false) {
  $("progressFill").style.width = indeterminate ? "" : `${Math.round(value * 100)}%`;
  $("progressFill").parentElement.classList.toggle("indeterminate", indeterminate);
  $("progressPct").textContent = indeterminate ? "" : `${Math.round(value * 100)}%`;
}

function showError(text, action) {
  $("alertText").textContent = text;
  $("alert").hidden = false;
  const btn = $("alertAction");
  if (action) {
    btn.textContent = action.label;
    btn.onclick = () => { hideError(); action.run(); };
    btn.hidden = false;
  } else {
    btn.hidden = true;
  }
}
function hideError() { $("alert").hidden = true; }

// Ошибки микрофона приходят с префиксом «mic:» — к ним добавляем кнопку,
// открывающую системные настройки доступа к микрофону.
function openMicSettings() {
  invoke("open_mic_settings").catch((e) => showError(String(e)));
}
function showMicAwareError(e) {
  const msg = String(e);
  if (msg.startsWith("mic:")) {
    showError(msg.slice(4), IS_LINUX ? null : { label: t("micSettings"), run: openMicSettings });
  } else {
    showError(msg);
  }
}

function appendText(text) {
  if (!text) return;
  const area = $("transcript");
  area.value = area.value ? area.value + "\n\n" + text : text;
  area.scrollTop = area.scrollHeight;
  renderBusy();
}

// ---------- Настройки ----------

async function saveSettings() {
  await invoke("save_settings", { settings: S.settings });
}

// ---------- Расшифровка ----------

function hasModel() {
  return !!S.settings.model && S.installed.some((m) => m.id === S.settings.model);
}

async function transcribe(paths) {
  if (!paths.length || S.busy || S.recording) return;
  if (!hasModel()) { openModels(); return; }
  hideError();
  try {
    S.busy = true; renderBusy(); setProgress(0, true);
    await invoke("transcribe", { paths });
  } catch (e) {
    S.busy = false; renderBusy();
    showError(String(e));
  }
}

async function chooseFiles() {
  if (S.busy) return;
  if (!hasModel()) { openModels(); return; }
  const picked = await T.dialog.open({
    multiple: true,
    title: t("openTitle"),
    filters: [{ name: t("mediaFilter"), extensions: MEDIA_EXT }, { name: t("allFiles"), extensions: ["*"] }],
  });
  if (!picked) return;
  transcribe(Array.isArray(picked) ? picked : [picked]);
}

function onJob(ev) {
  switch (ev.kind) {
    case "file":
      S.queue = ev.total > 1 ? t("fileOf", ev.index + 1, ev.total) : "";
      setProgress(0, true);
      renderStatus();
      break;
    case "phase":
      S.audioSeconds = ev.audioSeconds;
      if (ev.phase === "decoding") setStatus(() => t("reading", ev.name));
      else if (ev.phase === "loading") setStatus(() => t("loadingModel"));
      else if (ev.phase === "transcribing") {
        setStatus(() => ev.recording ? t("transcribingRec", timecode(ev.audioSeconds)) : t("transcribing", ev.name, timecode(ev.audioSeconds)));
        setProgress(0);
      }
      break;
    case "progress":
      setProgress(ev.value);
      break;
    case "result": {
      appendText(ev.header ? `=== ${ev.name} ===\n${ev.text}` : ev.text);
      if (ev.savedTo) S.lastSaved = ev.savedTo;
      S.lastOnGpu = ev.onGpu;
      renderEngineInfo();
      const speed = ev.elapsed > 0 ? (ev.audioSeconds / ev.elapsed).toFixed(1) : "—";
      const savedName = ev.savedTo ? baseName(ev.savedTo) : null;
      setStatus(() => t("done", duration(ev.elapsed), speed)
        + (ev.language ? t("langDetected", ev.language) : "")
        + (ev.onGpu || S.system.gpuBackend === "CPU" ? "" : t("onCpu"))
        + (savedName ? t("savedTo", savedName) : ""));
      renderBusy();
      break;
    }
    case "error":
      if (ev.noModel) {
        openModels();
      } else {
        showError(`${ev.name}: ${ev.message}`);
      }
      setStatus(() => t("ready"));
      break;
    case "done":
      S.busy = false;
      S.queue = "";
      if (ev.cancelled) setStatus(() => t("cancelled"));
      renderBusy();
      renderStatus();
      break;
  }
}

// ---------- Диктовка ----------

function dictationBaseName() {
  const d = new Date(), p = (n) => String(n).padStart(2, "0");
  return `${t("dictationPrefix")} ${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}-${p(d.getMinutes())}-${p(d.getSeconds())}`;
}

async function toggleRecording() {
  if (S.recording) {
    S.recording = false;
    renderBusy();
    try {
      S.busy = true; renderBusy(); setProgress(0, true);
      await invoke("stop_recording", { baseName: dictationBaseName() });
    } catch (e) {
      S.busy = false; renderBusy();
      showMicAwareError(e);
      setStatus(() => t("ready"));
    }
    return;
  }
  if (S.busy) return;
  if (!hasModel()) { openModels(); return; }
  hideError();
  try {
    await invoke("start_recording");
    S.recording = true;
    $("recTime").textContent = "00:00";
    $("recLevel").style.width = "2%";
    $("recWarn").hidden = true;
    setStatus(() => t("recording"));
    renderBusy();
  } catch (e) {
    showMicAwareError(e);
  }
}

// ---------- Выбор микрофона ----------

async function refreshMics() {
  let mics = [];
  try { mics = await invoke("list_mics"); } catch { /* нет звуковой подсистемы — просто прячем выбор */ }
  const sel = $("micSelect");
  const def = mics.find((m) => m.isDefault);
  sel.innerHTML = "";
  const first = document.createElement("option");
  first.value = ""; first.textContent = t("micDefault", def ? def.name : "");
  sel.appendChild(first);
  for (const m of mics) {
    const o = document.createElement("option");
    o.value = m.id; o.textContent = m.name;
    sel.appendChild(o);
  }
  const saved = S.settings.mic;
  sel.value = saved && mics.some((m) => m.id === saved) ? saved : "";
  // Выбор нужен, только когда микрофонов больше одного (или выбранный пропал).
  $("micWrap").hidden = mics.length < 2 && !saved;
}

// ---------- Модели ----------

function openModels() {
  renderModels();
  const dlg = $("modelsDialog");
  if (!dlg.open) dlg.showModal();
  // Модель могли положить в папку руками — перечитываем список.
  invoke("list_models").then(onModelsChanged).catch(() => {});
}

function isFirstRun() { return S.installed.length === 0; }

function dots(n) {
  return `<span class="dots">${[1, 2, 3, 4, 5].map((i) => `<i class="${i <= n ? "on" : ""}"></i>`).join("")}</span>`;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

function renderModels() {
  const sys = S.system;
  const first = isFirstRun();
  $("modelsHeading").textContent = first ? t("welcomeTitle") : t("modelsTitle");
  $("modelsIntro").textContent = first ? t("welcomeIntro") : t("modelsIntro");

  const gpuText = sys.gpuBackend === "CPU" ? t("gpuNone") : t("gpuVia", sys.gpuBackend, sys.gpuDevices[0] || "");
  $("machine").textContent = t("machine", t("cores", sys.cores), Math.round(sys.totalRamGb), gpuText);

  const hasGpu = sys.gpuBackend !== "CPU";
  $("useGpu").checked = S.settings.useGpu && hasGpu;
  $("useGpu").disabled = !hasGpu;
  $("useGpuLabel").textContent = hasGpu ? t("useGpu", sys.gpuBackend) : t("useGpuNone");

  const list = $("modelList");
  list.innerHTML = "";
  const installedIds = new Set(S.installed.map((m) => m.id));

  for (const m of S.catalog) {
    list.appendChild(modelCard({
      id: m.id, size: m.size, quality: m.quality, speed: m.speed, ramGb: m.ramGb,
      desc: MODEL_TEXT[m.id]?.[lang()] ?? "",
      installed: installedIds.has(m.id), custom: false,
      recommended: m.id === sys.recommended,
    }));
  }

  const custom = S.installed.filter((m) => m.custom);
  if (custom.length) {
    const label = document.createElement("div");
    label.className = "section-label";
    label.textContent = t("customModels");
    list.appendChild(label);
    for (const m of custom) {
      list.appendChild(modelCard({ id: m.id, size: m.size, desc: m.file, installed: true, custom: true }));
    }
  }
}

function modelCard(m) {
  const el = document.createElement("div");
  const active = S.settings.model === m.id && m.installed;
  el.className = "model" + (active ? " active" : "") + (m.recommended ? " recommended" : "");

  const badges = [];
  if (m.recommended) badges.push(`<span class="badge">★ ${esc(t("recommended"))}</span>`);
  if (active) badges.push(`<span class="badge ok">✓ ${esc(t("active"))}</span>`);

  let html = `
    <div class="model-title"><strong>${esc(modelName(m.id))}</strong>
      <span class="model-size">${esc(bytes(m.size))}</span>${badges.join("")}</div>
    <div class="model-desc">${esc(m.desc)}</div>`;
  if (!m.custom) {
    html += `<div class="model-meta">
      <span>${esc(t("quality"))}${dots(m.quality)}</span>
      <span>${esc(t("speed"))}${dots(m.speed)}</span>
      <span>${esc(t("ram", m.ramGb))}</span></div>`;
  }

  const downloading = S.downloading.has(m.id);
  const p = S.dl[m.id];
  html += `<div class="model-actions"><div class="row">`;
  if (downloading) {
    html += `<button class="btn small" data-act="cancel">${esc(t("cancel"))}</button>`;
  } else if (m.installed) {
    if (!active) html += `<button class="btn small primary" data-act="use">${esc(t("use"))}</button>`;
    html += `<button class="icon-btn" data-act="delete" title="${esc(t("delete"))}">
      <svg class="ico" viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/></svg></button>`;
  } else {
    const label = p && p.downloaded > 0 && p.phase !== "done" ? t("resume")
      : (m.recommended && isFirstRun() ? t("downloadRec", bytes(m.size)) : t("download"));
    html += `<button class="btn small ${m.recommended ? "primary" : ""}" data-act="download">${esc(label)}</button>`;
  }
  html += `</div></div>`;

  if (downloading) {
    const done = p?.downloaded ?? 0, total = p?.total ?? m.size;
    const pct = total ? Math.min(100, (done / total) * 100) : 0;
    let line;
    if (p?.phase === "verifying") line = t("verifying");
    else {
      line = t("dlProgress", bytes(done), bytes(total), p?.bytesPerSec ? bytes(p.bytesPerSec) : "");
      if (p?.bytesPerSec > 0) line += t("eta", timecode((total - done) / p.bytesPerSec));
    }
    html += `<div class="dl"><div class="bar ${p?.phase === "verifying" || !p ? "indeterminate" : ""}"><div class="bar-fill" style="width:${pct}%"></div></div><span class="mono">${esc(line)}</span></div>`;
  }
  if (S.dlErrors[m.id]) html += `<div class="model-error">${esc(S.dlErrors[m.id])}</div>`;

  el.innerHTML = html;
  el.querySelectorAll("[data-act]").forEach((b) => b.addEventListener("click", () => modelAction(b.dataset.act, m)));
  return el;
}

async function modelAction(act, m) {
  try {
    if (act === "download") {
      delete S.dlErrors[m.id];
      S.downloading.add(m.id);
      renderModels();
      await invoke("download_model", { id: m.id });
    } else if (act === "cancel") {
      await invoke("cancel_download", { id: m.id });
    } else if (act === "use") {
      selectModel(m.id);
    } else if (act === "delete") {
      const ok = await T.dialog.ask(t("deleteAsk", modelName(m.id)), { title: "Auris Whisper", kind: "warning" });
      if (!ok) return;
      await invoke("delete_model", { id: m.id });
      if (S.settings.model === m.id) {
        S.settings.model = null;
        await saveSettings();
      }
    }
  } catch (e) {
    S.dlErrors[m.id] = String(e);
    renderModels();
  }
}

async function selectModel(id) {
  S.settings.model = id;
  await saveSettings();
  renderModelChip();
  renderEngineInfo();
  renderModels();
}

function onModelsChanged(state) {
  const before = new Set(S.installed.map((m) => m.id));
  S.installed = state.installed;
  S.downloading = new Set(state.downloading);
  // Только что скачанная модель становится активной, если активной ещё нет.
  const fresh = S.installed.find((m) => !before.has(m.id));
  if (!hasModel()) {
    const pick = fresh ?? S.installed.find((m) => m.id === S.system.recommended) ?? S.installed[0];
    if (pick) selectModel(pick.id);
    else { S.settings.model = null; }
  }
  renderModelChip();
  renderEngineInfo();
  if ($("modelsDialog").open) renderModels();
}

function onDownloadProgress(p) {
  S.dl[p.id] = p;
  if (p.phase === "error") {
    S.dlErrors[p.id] = p.error;
    S.downloading.delete(p.id);
  }
  if ($("modelsDialog").open) renderModels();
}

// ---------- Полоса нагрузки ----------

function barColor(v) { return v < 0.6 ? "var(--ok)" : v < 0.85 ? "var(--mid)" : "var(--hot)"; }

function setGauge(id, frac, text) {
  const el = $(id);
  const b = el.querySelector("b");
  if (b) {
    const v = Math.min(1, Math.max(0, frac));
    b.style.width = `${Math.max(2, v * 100)}%`;
    b.style.background = barColor(v);
  }
  el.querySelector("em").textContent = text;
}

function onStats(s) {
  setGauge("gApp", s.appCpu / (s.cores * 100), `${Math.round(s.appCpu)}%`);
  setGauge("gSys", s.systemCpu / 100, `${Math.round(s.systemCpu)}%`);
  $("gGpu").hidden = s.gpu == null;
  if (s.gpu != null) setGauge("gGpu", s.gpu / 100, `${Math.round(s.gpu)}%`);
  const mem = s.memoryMb >= 1024 ? `${(s.memoryMb / 1024).toFixed(2)} ${t("gb")}` : `${Math.round(s.memoryMb)} ${t("mb")}`;
  setGauge("gMem", 0, mem);
}

// ---------- О программе ----------

function openAbout() {
  $("aboutVersion").textContent = t("aboutVersion", S.system.version, S.system.whisperVersion);
  $("aboutText").textContent = t("aboutText");
  $("aboutCredits").textContent = t("aboutCredits");
  $("aboutDialog").showModal();
}

// ---------- Подключение ----------

function wire() {
  $("language").addEventListener("change", (e) => { S.settings.language = e.target.value; saveSettings(); });
  $("timestamps").addEventListener("change", (e) => { S.settings.timestamps = e.target.checked; saveSettings(); });
  $("autoSave").addEventListener("change", (e) => { S.settings.autoSave = e.target.checked; saveSettings(); });
  document.querySelectorAll("#uiLang button").forEach((b) => b.addEventListener("click", () => {
    S.settings.uiLang = b.dataset.lang;
    saveSettings();
    applyStrings();
  }));

  $("modelChip").addEventListener("click", openModels);
  $("engineInfo").addEventListener("click", openModels);
  $("modelsClose").addEventListener("click", () => $("modelsDialog").close());
  $("useGpu").addEventListener("change", (e) => { S.settings.useGpu = e.target.checked; S.lastOnGpu = null; saveSettings(); renderEngineInfo(); });
  $("modelsDirBtn").addEventListener("click", () => invoke("open_models_dir").catch((e) => showError(String(e))));
  $("importBtn").addEventListener("click", async () => {
    const picked = await T.dialog.open({ multiple: false, title: t("importTitle"), filters: [{ name: "ggml", extensions: ["bin"] }] });
    if (!picked) return;
    try {
      const id = await invoke("import_model", { path: picked });
      await selectModel(id);
    } catch (e) {
      $("machine").textContent = String(e);
    }
  });

  $("openBtn").addEventListener("click", chooseFiles);
  $("dictateBtn").addEventListener("click", toggleRecording);
  $("stopBtn").addEventListener("click", toggleRecording);
  $("cancelBtn").addEventListener("click", () => { invoke("cancel_job"); setStatus(() => t("cancelling")); });
  $("alertClose").addEventListener("click", hideError);

  $("clearBtn").addEventListener("click", () => {
    $("transcript").value = "";
    S.lastSaved = null;
    hideError();
    setStatus(() => t("ready"));
    renderBusy();
  });
  $("copyBtn").addEventListener("click", async () => {
    try { await invoke("copy_text", { text: $("transcript").value }); setStatus(() => t("copied")); }
    catch (e) { showError(String(e)); }
  });
  $("saveBtn").addEventListener("click", saveAs);
  $("revealBtn").addEventListener("click", () => S.lastSaved && invoke("reveal", { path: S.lastSaved }).catch((e) => showError(String(e))));
  $("transcript").addEventListener("input", renderBusy);

  $("aboutBtn").addEventListener("click", openAbout);
  $("aboutClose").addEventListener("click", () => $("aboutDialog").close());
  $("githubBtn").addEventListener("click", () => invoke("open_url", { url: "https://github.com/sibawal/auris-whisper" }));

  // Горячие клавиши: ⌘/Ctrl + O, R, S
  document.addEventListener("keydown", (e) => {
    const mod = IS_MAC ? e.metaKey : e.ctrlKey;
    if (S.recording && e.key === "Enter" && !e.isComposing) { e.preventDefault(); toggleRecording(); return; }
    if (!mod || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === "o" || k === "щ") { e.preventDefault(); chooseFiles(); }
    else if (k === "r" || k === "к") { e.preventDefault(); toggleRecording(); }
    else if (k === "s" || k === "ы") { e.preventDefault(); if ($("transcript").value) saveAs(); }
  });
  // Не даём вебвью открыть брошенный файл как страницу
  window.addEventListener("dragover", (e) => e.preventDefault());
  window.addEventListener("drop", (e) => e.preventDefault());
  document.addEventListener("contextmenu", (e) => {
    if (!e.target.closest("textarea, .status, .alert")) e.preventDefault();
  });

  // Перетаскивание файлов: Tauri отдаёт настоящие пути
  T.webview.getCurrentWebview().onDragDropEvent((ev) => {
    const p = ev.payload;
    const drop = $("drop");
    if (p.type === "enter" || p.type === "over") drop.classList.toggle("over", !S.busy && !S.recording);
    else if (p.type === "leave") drop.classList.remove("over");
    else if (p.type === "drop") {
      drop.classList.remove("over");
      transcribe(p.paths || []);
    }
  });

  listen("job", (e) => onJob(e.payload));
  $("micSelect").addEventListener("change", (e) => { S.settings.mic = e.target.value || null; saveSettings(); });
  $("micSelect").addEventListener("focus", refreshMics);
  window.addEventListener("focus", () => { if (!S.recording) refreshMics(); });
  $("recWarnBtn").addEventListener("click", openMicSettings);
  if (IS_LINUX) $("recWarnBtn").hidden = true;

  listen("rec-level", (e) => {
    $("recWarn").hidden = !e.payload.silent;
    $("recTime").textContent = timecode(e.payload.seconds);
    $("recLevel").style.width = `${Math.max(2, e.payload.level * 100)}%`;
  });
  listen("download-progress", (e) => onDownloadProgress(e.payload));
  listen("models-changed", (e) => onModelsChanged(e.payload));
  listen("stats", (e) => onStats(e.payload));
  listen("open-files", (e) => transcribe(e.payload));
}

async function saveAs() {
  const text = $("transcript").value;
  if (!text) return;
  const path = await T.dialog.save({ defaultPath: t("transcriptFile"), filters: [{ name: t("textFilter"), extensions: ["txt"] }] });
  if (!path) return;
  try {
    await invoke("save_text", { path, text });
    S.lastSaved = path;
    setStatus(() => t("saved", baseName(path)));
    renderBusy();
  } catch (e) {
    showError(String(e));
  }
}

async function start() {
  const b = await invoke("bootstrap");
  S.settings = b.settings;
  S.catalog = b.catalog;
  S.installed = b.installed;
  S.downloading = new Set(b.downloading);
  S.system = b.system;
  S.modelsDir = b.modelsDir;

  // Первый запуск: язык интерфейса — по языку системы
  if (!S.settings.uiLang) {
    const sysLang = (navigator.language || "en").toLowerCase();
    S.settings.uiLang = /^(ru|uk|be|kk|ky|uz|tg|hy|az|ka)/.test(sysLang) ? "ru" : "en";
    await saveSettings();
  }
  // Выбранная модель пропала (удалили руками) — берём любую установленную
  if (!hasModel() && S.installed.length) {
    const pick = S.installed.find((m) => m.id === S.system.recommended) ?? S.installed[0];
    S.settings.model = pick.id;
    await saveSettings();
  }

  $("timestamps").checked = S.settings.timestamps;
  $("autoSave").checked = S.settings.autoSave;
  wire();
  applyStrings();
  renderBusy();

  if (!hasModel()) openModels();
  else if (b.pendingFiles.length) transcribe(b.pendingFiles);
}

start().catch((e) => {
  document.body.textContent = "Auris Whisper: " + e;
});
