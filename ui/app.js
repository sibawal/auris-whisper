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
  diarizeReady: false,
  speakers: [],    // {label, name} — номера сквозные на весь текст в окне
  results: [],     // {name, header, segments, timestamps, savePath} — для переименования
  update: null,    // найденное обновление
};

const SPEAKER_COLORS = ["#5b5ce6", "#e5484d", "#30a46c", "#f76b15", "#0090ff", "#ab4aba", "#d6409f", "#978365"];

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

// Модель понимает только русский (GigaAM, T-One).
function isRuModel(id) { return S.catalog.find((m) => m.id === id)?.lang === "ru"; }
function ruLocked() { return hasModel() && isRuModel(S.settings.model); }

function modelName(id) {
  const m = MODEL_TEXT[id];
  return m?.[`name_${lang()}`] ?? m?.name ?? id;
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
  renderLanguageLock();

  document.querySelectorAll("#uiLang button").forEach((b) => b.classList.toggle("on", b.dataset.lang === lang()));
  renderStatus();
  renderModelChip();
  renderEngineInfo();
  if ($("modelsDialog").open) renderModels();
  refreshMics();
  renderSpeakerCount();
  renderDiarize();
  renderUpdate();
  renderRuTip();
}

// С русской моделью язык один — показываем его и не даём менять.
function renderLanguageLock() {
  const sel = $("language"), locked = ruLocked();
  sel.value = locked ? "ru" : S.settings.language;
  sel.disabled = S.busy || locked;
  sel.title = locked ? t("ruLangLocked") : "";
}

// Выбран русский язык, а считает Whisper — подсказываем, что есть модели лучше.
function renderRuTip() {
  $("ruTip").hidden = !(S.settings.language === "ru" && hasModel() && !isRuModel(S.settings.model) && !S.settings.ruTipHidden);
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
  const gpu = S.settings.useGpu && sys.gpuBackend !== "CPU" && S.lastOnGpu !== false && !ruLocked() ? sys.gpuBackend : "CPU";
  $("engineInfo").textContent = gpu === "CPU" ? (sys.compat ? t("cpuCompat") : "CPU") : `⚡ ${gpu}`;
  $("engineInfo").title = (gpu === "CPU" ? t("gpuNone") : sys.gpuDevices.join(", ")) + " — " + t("modelsTitle");
}

function renderBusy() {
  const busy = S.busy, rec = S.recording;
  $("progressRow").hidden = !busy;
  $("openBtn").disabled = busy;
  $("dictateBtn").disabled = busy;
  renderLanguageLock();
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

// ---------- Текст из сегментов и спикеры ----------

function speakerName(i) {
  const sp = S.speakers[i];
  return sp ? (sp.name || sp.label) : "";
}

/// Текст одного результата: с таймкодами — строка на фразу, без них —
/// абзац на реплику спикера.
function formatSegments(segs, timestamps) {
  const label = (s) => (s.speaker != null ? `${speakerName(s.speaker)}: ` : "");
  if (timestamps) {
    return segs.map((s) => `[${timecode(s.start)} → ${timecode(s.end)}]  ${label(s)}${s.text}`).join("\n");
  }
  if (!segs.some((s) => s.speaker != null)) {
    return segs.map((s) => s.text).join(" ").replace(/ {2,}/g, " ");
  }
  const turns = [];
  for (const s of segs) {
    const last = turns[turns.length - 1];
    if (last && last.speaker === s.speaker) last.text += " " + s.text;
    else turns.push({ speaker: s.speaker, text: s.text });
  }
  return turns.map((x) => label(x) + x.text).join("\n\n");
}

async function onResult(ev) {
  // Номера спикеров сквозные: у второго файла — следующие после первого.
  const base = S.speakers.length;
  for (let i = 0; i < ev.speakers; i++) S.speakers.push({ label: t("speakerN", base + i + 1), name: "" });
  const segments = ev.segments.map((x) => ({ ...x, speaker: x.speaker == null ? null : base + x.speaker }));
  const r = { name: ev.name, header: ev.header, segments, timestamps: S.settings.timestamps, savePath: ev.savePath };
  S.results.push(r);

  const body = formatSegments(segments, r.timestamps);
  appendText(r.header ? `=== ${r.name} ===\n${body}` : body);
  renderSpeakers();

  let saved = null;
  if (r.savePath && body) {
    try { await invoke("save_text", { path: r.savePath, text: body }); saved = r.savePath; S.lastSaved = saved; }
    catch (e) { showError(String(e)); }
  }
  S.lastOnGpu = ev.onGpu;
  renderEngineInfo();
  const speed = ev.elapsed > 0 ? (ev.audioSeconds / ev.elapsed).toFixed(1) : "—";
  setStatus(() => t("done", duration(ev.elapsed), speed)
    + (ev.language ? t("langDetected", ev.language) : "")
    + (ev.onGpu || S.system.gpuBackend === "CPU" ? "" : t("onCpu"))
    + (saved ? t("savedTo", baseName(saved)) : ""));
  renderBusy();
}

function escapeRe(s) { return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"); }

/// Новое имя спикера заменяет старое везде: в окне (подписи реплик, в том числе
/// с таймкодами) и в уже сохранённых .txt.
async function renameSpeaker(i, raw) {
  const sp = S.speakers[i];
  if (!sp) return;
  const before = speakerName(i);
  const wanted = raw.trim().replace(/:/g, "");
  const after = wanted || sp.label;
  if (after === before) { renderSpeakers(); return; }
  if (S.speakers.some((_, j) => j !== i && speakerName(j) === after)) {
    showError(t("speakerNameTaken", after));
    renderSpeakers();
    return;
  }
  sp.name = wanted;
  const area = $("transcript");
  const re = new RegExp(`(^|\\n)((?:\\[[^\\]\\n]*\\]\\s+)?)${escapeRe(before)}:`, "g");
  area.value = area.value.replace(re, (_, nl, ts) => `${nl}${ts}${after}:`);
  renderSpeakers();
  for (const r of S.results) {
    if (r.savePath && r.segments.some((x) => x.speaker === i)) {
      try { await invoke("save_text", { path: r.savePath, text: formatSegments(r.segments, r.timestamps) }); }
      catch (e) { showError(String(e)); }
    }
  }
}

function renderSpeakers() {
  const box = $("speakerChips");
  box.innerHTML = "";
  $("speakers").hidden = S.speakers.length === 0;
  S.speakers.forEach((sp, i) => {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "speaker-chip";
    chip.innerHTML = `<i style="background:${SPEAKER_COLORS[i % SPEAKER_COLORS.length]}"></i><span></span>
      <svg class="ico" viewBox="0 0 24 24"><path d="M4 20h4L19 9l-4-4L4 16v4z"/></svg>`;
    chip.querySelector("span").textContent = speakerName(i);
    chip.addEventListener("click", () => editSpeaker(chip, i));
    box.appendChild(chip);
  });
}

function editSpeaker(chip, i) {
  const span = chip.querySelector("span");
  const input = document.createElement("input");
  input.value = S.speakers[i].name;
  input.placeholder = S.speakers[i].label;
  span.replaceWith(input);
  chip.querySelector("svg")?.remove();
  input.focus();
  input.select();
  let done = false;
  const finish = (save) => {
    if (done) return;
    done = true;
    if (save) renameSpeaker(i, input.value); else renderSpeakers();
  };
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); finish(true); }
    else if (e.key === "Escape") { e.preventDefault(); finish(false); }
    e.stopPropagation();
  });
  input.addEventListener("blur", () => finish(true));
  input.addEventListener("click", (e) => e.stopPropagation());
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

/// Включено разделение по голосам, а модели ещё качаются — ждём.
function diarizeBlocked() {
  if (!S.settings.diarize || S.diarizeReady) return false;
  showError(t("diarizeWait"));
  if (!S.downloading.has("diarization")) startDiarizeDownload();
  return true;
}

async function transcribe(paths) {
  if (!paths.length || S.busy || S.recording) return;
  if (!hasModel()) { openModels(); return; }
  if (diarizeBlocked()) return;
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
      else if (ev.phase === "diarizing") {
        setStatus(() => ev.recording ? t("diarizingRec") : t("diarizing", ev.name));
        setProgress(0);
      }
      else if (ev.phase === "transcribing") {
        setStatus(() => ev.recording ? t("transcribingRec", timecode(ev.audioSeconds)) : t("transcribing", ev.name, timecode(ev.audioSeconds)));
        setProgress(0);
      }
      break;
    case "progress":
      setProgress(ev.value);
      break;
    case "result":
      onResult(ev);
      break;
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
  if (diarizeBlocked()) return;
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
  return invoke("list_models").then(onModelsChanged).catch(() => {});
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

  const card = (m) => modelCard({
    id: m.id, size: m.size, quality: m.quality, speed: m.speed, ramGb: m.ramGb,
    desc: MODEL_TEXT[m.id]?.[lang()] ?? "",
    installed: installedIds.has(m.id), custom: false,
    recommended: m.id === sys.recommended,
    ruBest: m.id === "gigaam-v3",
  });
  for (const m of S.catalog.filter((m) => !m.lang)) list.appendChild(card(m));

  const ru = document.createElement("div");
  ru.className = "section-label";
  ru.id = "ruSection";
  ru.textContent = t("ruOnly");
  list.appendChild(ru);
  const ruIntro = document.createElement("div");
  ruIntro.className = "section-intro";
  ruIntro.textContent = t("ruOnlyIntro");
  list.appendChild(ruIntro);
  for (const m of S.catalog.filter((m) => m.lang === "ru")) list.appendChild(card(m));

  const extras = document.createElement("div");
  extras.className = "section-label";
  extras.textContent = t("extras");
  list.appendChild(extras);
  list.appendChild(modelCard({
    id: "diarization", size: 34274077, desc: MODEL_TEXT.diarization[lang()],
    installed: S.diarizeReady, custom: true, extra: true,
  }));

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
  const active = !m.extra && S.settings.model === m.id && m.installed;
  el.className = "model" + (active ? " active" : "") + (m.recommended ? " recommended" : "");

  const badges = [];
  if (m.recommended) badges.push(`<span class="badge">★ ${esc(t("recommended"))}</span>`);
  if (m.ruBest) badges.push(`<span class="badge">★ ${esc(t("ruBadge"))}</span>`);
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
    if (!active && !m.extra) html += `<button class="btn small primary" data-act="use">${esc(t("use"))}</button>`;
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
      if (m.id === "diarization") await invoke("download_diarization");
      else await invoke("download_model", { id: m.id });
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
  renderLanguageLock();
  renderRuTip();
  renderModels();
}

function onModelsChanged(state) {
  const before = new Set(S.installed.map((m) => m.id));
  S.installed = state.installed;
  S.downloading = new Set(state.downloading);
  S.diarizeReady = state.diarizeReady;
  renderDiarize();
  // Только что скачанная модель становится активной, если активной ещё нет.
  const fresh = S.installed.find((m) => !before.has(m.id));
  if (!hasModel()) {
    const pick = fresh ?? S.installed.find((m) => m.id === S.system.recommended) ?? S.installed[0];
    if (pick) selectModel(pick.id);
    else { S.settings.model = null; }
  }
  renderModelChip();
  renderEngineInfo();
  renderLanguageLock();
  renderRuTip();
  if ($("modelsDialog").open) renderModels();
}

function onDownloadProgress(p) {
  S.dl[p.id] = p;
  if (p.id === "diarization") renderDiarize();
  if (p.phase === "error") {
    S.dlErrors[p.id] = p.error;
    S.downloading.delete(p.id);
  }
  if ($("modelsDialog").open) renderModels();
}

// ---------- Разделение по голосам ----------

function renderDiarize() {
  const on = !!S.settings?.diarize;
  $("diarize").checked = on;
  $("speakerCount").hidden = !on;
  const p = S.dl.diarization;
  let note = "";
  if (on && !S.diarizeReady) {
    if (S.downloading.has("diarization") && p && p.total) note = t("diarizeDownloading", Math.floor((p.downloaded / p.total) * 100));
    else if (S.dlErrors.diarization) note = t("diarizeFailed", S.dlErrors.diarization);
  }
  $("diarizeStatus").textContent = note;
}

function renderSpeakerCount() {
  const sel = $("speakerCount");
  sel.innerHTML = "";
  const auto = document.createElement("option");
  auto.value = ""; auto.textContent = t("speakersAuto");
  sel.appendChild(auto);
  for (let n = 2; n <= 8; n++) {
    const o = document.createElement("option");
    o.value = String(n); o.textContent = t("speakersCount", n);
    sel.appendChild(o);
  }
  sel.value = S.settings.speakers ? String(S.settings.speakers) : "";
}

async function startDiarizeDownload() {
  delete S.dlErrors.diarization;
  S.downloading.add("diarization");
  renderDiarize();
  try { await invoke("download_diarization"); }
  catch (e) { S.dlErrors.diarization = String(e); S.downloading.delete("diarization"); renderDiarize(); }
}

// ---------- Обновления ----------

async function checkUpdate(manual = false) {
  try {
    const info = await invoke("check_update");
    if (info) {
      S.update = info;
      renderUpdate();
    }
    if (manual) $("updateCheckResult").textContent = info ? t("updateAvailable", info.version, info.current) : t("upToDate");
  } catch (e) {
    if (manual) $("updateCheckResult").textContent = t("updateCheckFailed");
  }
}

function renderUpdate() {
  const u = S.update;
  $("updateBar").hidden = !u;
  if (!u) return;
  $("updateText").textContent = t("updateAvailable", u.version, u.current);
  $("updateInstallBtn").textContent = u.canInstall ? t("updateInstall") : t("updateDownload");
  $("updateNotesBtn").hidden = !u.notes;
}

async function installUpdate() {
  const u = S.update;
  if (!u) return;
  if (!u.canInstall) { invoke("open_url", { url: u.page }); return; }
  if (S.busy || S.recording) return;
  $("updateInstallBtn").disabled = true;
  $("updateProgress").hidden = false;
  $("updateText").textContent = t("updateDownloading", "");
  try {
    await invoke("install_update");
    // Сюда обычно не доходим: после установки приложение перезапускается.
  } catch (e) {
    $("updateProgress").hidden = true;
    $("updateInstallBtn").disabled = false;
    showError(t("updateFailed", String(e)));
    renderUpdate();
  }
}

function onUpdateProgress(p) {
  const pct = p.total ? Math.floor((p.downloaded / p.total) * 100) : null;
  $("updateProgressFill").style.width = pct != null ? `${pct}%` : "30%";
  $("updateText").textContent = pct != null && pct >= 100 ? t("updateInstalling")
    : t("updateDownloading", pct != null ? `${pct}%` : bytes(p.downloaded));
}

function showNotes() {
  const u = S.update;
  if (!u) return;
  $("notesTitle").textContent = t("whatsNew", u.version);
  // Markdown показываем как текст, убрав разметку.
  $("notesBody").textContent = u.notes.replace(/\*\*|`|^#+\s*/gm, "").replace(/\|/g, " ").replace(/^\s*-{3,}.*$/gm, "");
  $("notesDialog").showModal();
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
  $("checkUpdates").checked = S.settings.checkUpdates !== false;
  $("updateCheckResult").textContent = "";
  $("aboutVersion").textContent = t("aboutVersion", S.system.version, S.system.whisperVersion);
  $("aboutText").textContent = t("aboutText");
  $("aboutCredits").textContent = t("aboutCredits");
  $("aboutDialog").showModal();
}

// ---------- Подключение ----------

function wire() {
  $("language").addEventListener("change", (e) => { S.settings.language = e.target.value; saveSettings(); renderRuTip(); });
  $("ruTipBtn").addEventListener("click", async () => {
    await openModels(); // список перерисуется — прокручиваем уже после этого
    $("ruSection")?.scrollIntoView({ block: "start" });
  });
  $("ruTipClose").addEventListener("click", () => { S.settings.ruTipHidden = true; saveSettings(); renderRuTip(); });
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
    S.speakers = [];
    S.results = [];
    renderSpeakers();
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
  $("diarize").addEventListener("change", (e) => {
    S.settings.diarize = e.target.checked;
    saveSettings();
    if (S.settings.diarize && !S.diarizeReady && !S.downloading.has("diarization")) startDiarizeDownload();
    renderDiarize();
  });
  $("speakerCount").addEventListener("change", (e) => {
    S.settings.speakers = e.target.value ? Number(e.target.value) : null;
    saveSettings();
  });
  $("updateInstallBtn").addEventListener("click", installUpdate);
  $("updateNotesBtn").addEventListener("click", showNotes);
  $("updateClose").addEventListener("click", () => { $("updateBar").hidden = true; });
  $("notesClose").addEventListener("click", () => $("notesDialog").close());
  $("checkUpdates").addEventListener("change", (e) => { S.settings.checkUpdates = e.target.checked; saveSettings(); });
  $("checkNowBtn").addEventListener("click", () => { $("updateCheckResult").textContent = "…"; checkUpdate(true); });
  listen("update-progress", (e) => onUpdateProgress(e.payload));

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
  S.diarizeReady = b.diarizeReady;

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
  if (S.settings.diarize && !S.diarizeReady && !S.downloading.has("diarization")) startDiarizeDownload();
  // Проверка обновлений — чуть позже, чтобы не мешать запуску.
  if (S.settings.checkUpdates !== false) setTimeout(() => checkUpdate(false), 3000);
}

start().catch((e) => {
  document.body.textContent = "Auris Whisper: " + e;
});
