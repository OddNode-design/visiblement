const $ = id => document.getElementById(id);
const win = $("win"), stage = $("stage");

// --- Bridge to the Rust side (src/app.rs)
const send = msg => window.ipc?.postMessage(JSON.stringify(msg)); // no-op outside the app (browser preview)
const cmd = (...a) => send({ t: "cmd", a: a.map(String) });
const setProp = (k, v) => send({ t: "set", k, v: String(v) });
const st = {}; // mirror of the observed mpv properties (strings, or parsed JSON for lists)
const num = k => { const v = parseFloat(st[k]); return Number.isFinite(v) ? v : 0; };
const json = k => { try { return JSON.parse(st[k] ?? "null"); } catch { return null; } };

// --- Lucide icons
const setSrc = el => el.style.setProperty("--src", `url(icons/${el.dataset.i}.svg)`);
document.querySelectorAll(".ico").forEach(setSrc);
function swapIcon(el, name) {
  if (el.dataset.i === name) return;
  el.dataset.i = name; setSrc(el);
  el.classList.remove("swap"); void el.offsetWidth; el.classList.add("swap");
}

const fmt = s => {
  s = Math.max(0, Math.floor(s || 0));
  const h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, sec = String(s % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
};

// --- State from mpv → render
let scrubbing = false;
window.vb = {
  update(changes) { Object.assign(st, changes); render(); },
  loading(on) { on ? startLoading() : stopLoading(); },
  toast: msg => toast(msg),
  drag(on) { stage.classList.toggle("dragging", on); },
  // Startup check found a newer GitHub release (src/main.rs check_for_update).
  // prompt false: "Ne plus demander" was ticked, so only the discreet button lights up.
  updateAvailable({ latest, current, prompt, installable }) {
    const v = latest.replace(/^v/, "");
    canInstall = !!installable;
    $("updateGo").textContent = canInstall ? "Installer" : "Télécharger";
    $("updateText").innerHTML = `Visiblement <b>${v}</b> est prête. Tu as la version ${current}.`;
    $("updBtn").classList.add("available");
    $("updText").textContent = `Mise à jour ${v} disponible`;
    if (prompt) { $("updateNever").checked = false; $("updateBack").classList.add("open"); }
  },
  // Download progress (0-100), then "install" while Windows Installer takes over.
  updateProgress(p) {
    document.querySelector(".modal").classList.add("installing");
    const bar = $("updateBar");
    if (p === "install") { bar.classList.add("busy"); $("updateLabel").textContent = "Installation… Visiblement va redémarrer"; }
    else { bar.classList.remove("busy"); bar.style.width = p + "%"; $("updateLabel").textContent = `Téléchargement… ${p} %`; }
  },
  updateFailed() {
    document.querySelector(".modal").classList.remove("installing");
    canInstall = false; $("updateGo").textContent = "Télécharger";
    $("updateBack").classList.add("open");
    toast("Échec de la mise à jour : télécharge-la depuis la page des versions");
  },
  version(v) { if (!$("updBtn").classList.contains("available")) $("updText").textContent = `Version ${v} · Rechercher une mise à jour`; },
  maximized(on) { win.classList.toggle("maximized", on); swapIcon($("wMax").querySelector(".ico"), on ? "copy" : "square"); },
  fullscreen(on) { win.classList.toggle("fullscreen", on); swapIcon($("fsIcon"), on ? "minimize" : "maximize"); },
};

function render() {
  const idle = st["idle-active"] === "yes";
  const audio = !idle && !json("current-tracks/video") && !!json("current-tracks/audio");
  stage.classList.toggle("idle", idle);
  stage.classList.toggle("audio", audio);

  const t = num("time-pos"), d = num("duration");
  const name = idle ? "Visiblement" : (st.filename || "Visiblement");
  if ($("title").textContent !== name) $("title").textContent = name;
  const count = num("playlist-count"), pos = num("playlist-pos");
  $("pos").textContent = !idle && count > 1 ? `${pos + 1} / ${count}` : "";
  const frame = st["estimated-frame-number"], frames = st["estimated-frame-count"];
  const hasFrames = !audio && frame != null && frames != null;
  $("time").textContent = fmt(t); $("dur").textContent = fmt(d);
  $("frameStat").hidden = !hasFrames;
  if (hasFrames) { $("frame").textContent = frame; $("frames").textContent = frames; }
  if (!scrubbing) $("fill").style.width = (d ? Math.min(1, t / d) * 100 : 0) + "%";

  const paused = st.pause === "yes";
  stage.classList.toggle("playing", !paused && !idle);
  swapIcon($("playIcon"), paused ? "play" : "pause");
  $("play").dataset.tip = paused ? "Lecture (Espace)" : "Pause (Espace)";

  renderVolume();
  renderMarks(d);

  const alpha = st.alpha === "true", tiles = st.background === "tiles";
  // Always available on pictures; the tooltip says whether this file really has transparency.
  $("alphaBtn").hidden = audio;
  $("alphaBtn").dataset.tip = alpha ? "Damier de transparence (T)" : "Damier de transparence (T) · pas d'alpha dans ce fichier";
  $("alphaBtn").classList.toggle("on", tiles);
  swapIcon($("alphaIcon"), tiles ? "grid-2x2-check" : "grid-2x2");
}

// Markers (WAV cue points / After Effects) and the B–N selection on the timeline.
let marksKey = "";
function renderMarks(d) {
  const chapters = json("chapter-list") || [];
  const key = JSON.stringify(chapters) + d;
  if (key !== marksKey) {
    marksKey = key;
    $("marks").replaceChildren(...chapters.filter(() => d > 0).map(c => {
      const el = document.createElement("div");
      el.className = "mark"; el.style.left = (c.time / d * 100) + "%";
      el.dataset.time = c.time;
      return el;
    }));
  }
  const a = parseFloat(st["ab-loop-a"]), b = parseFloat(st["ab-loop-b"]);
  const sel = $("sel");
  if (d > 0 && (Number.isFinite(a) || Number.isFinite(b))) {
    const from = Number.isFinite(a) ? a : 0, to = Number.isFinite(b) ? b : d;
    sel.style.display = "block"; sel.style.left = (from / d * 100) + "%"; sel.style.width = (Math.max(0, to - from) / d * 100) + "%";
  } else sel.style.display = "none";
}

// --- Mascot eyes: a spring chases the gaze target (slight overshoot = alive), wide range off-centre,
// random glances when the mouse rests, blinks now and then.
// While audio plays, the two eyes melt into a 7-bar waveform (and back on pause): every bar's
// position, size and roundness eases toward its target each frame, so the morph stays fluid.
const EYE_C = { x: 256, y: 256 }, RANGE = 44, BARS = 7, EYE_BARS = { 2: 226, 4: 286 };
const SVGNS = "http://www.w3.org/2000/svg";
const bars = Array.from({ length: BARS }, (_, i) => {
  const el = $("eyes").appendChild(document.createElementNS(SVGNS, "rect"));
  const eye = EYE_BARS[i];
  return { el, x: eye ?? 256, y: 256, w: eye ? 42 : 0, h: eye ? 68 : 0, r: 21, o: eye ? 1 : 0 };
});
let gaze = { ...EYE_C }, epos = { x: 0, y: 0 }, evel = { x: 0, y: 0 };
let lastMove = 0, nextGlance = 0, blinkUntil = 0, nextBlink = performance.now() + 2500;
addEventListener("mousemove", ev => {
  const r = $("mascot").getBoundingClientRect();
  if (!r.width) return;
  const s = 512 / r.width;
  gaze = { x: (ev.clientX - r.left) * s, y: (ev.clientY - r.top) * s };
  lastMove = performance.now();
});
// Organic bar heights: a few slow sines per bar, never in sync.
const waveHeight = (i, t) => {
  const s = t / 1000;
  const v = Math.sin(s * 3.1 + i * 1.7) * 0.5 + Math.sin(s * 5.3 + i * 0.9) * 0.3 + Math.sin(s * 1.3 + i * 2.3) * 0.2;
  const envelope = 1 - Math.abs(i - (BARS - 1) / 2) / BARS; // taller in the middle
  return 18 + (v * 0.5 + 0.5) * 70 * envelope;
};
(function eyeLoop(t) {
  if (stage.classList.contains("idle") || stage.classList.contains("audio")) {
    const wave = stage.classList.contains("audio") && stage.classList.contains("playing");
    if (t - lastMove > 2500 && t > nextGlance) {
      const a = Math.random() * Math.PI * 2, d = 120 + Math.random() * 400;
      gaze = Math.random() < 0.25 ? { ...EYE_C } : { x: EYE_C.x + Math.cos(a) * d, y: EYE_C.y + Math.sin(a) * d * 0.7 };
      nextGlance = t + 900 + Math.random() * 2200;
    }
    const dx = gaze.x - EYE_C.x, dy = gaze.y - EYE_C.y, d = Math.hypot(dx, dy) || 1, k = Math.min(d / 3, RANGE) / d;
    evel.x = (evel.x + (dx * k - epos.x) * 0.14) * 0.74;
    evel.y = (evel.y + (dy * k - epos.y) * 0.14) * 0.74;
    epos.x += evel.x; epos.y += evel.y;
    if (t > nextBlink) { blinkUntil = t + 140; nextBlink = t + 2200 + Math.random() * 3800; }
    const blink = t < blinkUntil;
    bars.forEach((b, i) => {
      const eye = EYE_BARS[i];
      const target = wave
        ? { x: 256 + (i - (BARS - 1) / 2) * 22, y: 262, w: 12, h: waveHeight(i, t), r: 6, o: 1 }
        : eye
          ? { x: eye + epos.x, y: 256 + epos.y, w: 42, h: blink ? 8 : 68, r: 21, o: 1 }
          : { x: 256 + epos.x, y: 256 + epos.y, w: 0, h: 0, r: 6, o: 0 };
      const ease = wave ? 0.18 : 0.12;
      for (const key of ["x", "y", "w", "h", "r", "o"]) b[key] += (target[key] - b[key]) * (key === "h" && blink && !wave ? 0.5 : ease);
      const r = Math.min(b.r, b.w / 2, b.h / 2);
      b.el.setAttribute("x", (b.x - b.w / 2).toFixed(2)); b.el.setAttribute("y", (b.y - b.h / 2).toFixed(2));
      b.el.setAttribute("width", Math.max(0, b.w).toFixed(2)); b.el.setAttribute("height", Math.max(0, b.h).toFixed(2));
      b.el.setAttribute("rx", Math.max(0, r).toFixed(2)); b.el.setAttribute("opacity", b.o.toFixed(3));
    });
  }
  requestAnimationFrame(eyeLoop);
})(0);

// White sphere for audio: its radius follows a few slow harmonics with drifting phases, so the
// outline keeps deforming organically. Filled with a soft radial white, blurred by CSS.
const mist = $("mist"), mctx = mist.getContext("2d"), MS = 256;
const HARMONICS = [[2, 0.075, 0.23], [3, 0.06, -0.17], [4, 0.04, 0.31], [5, 0.03, -0.27], [7, 0.018, 0.41]]; // [lobes, depth, speed]
let mistTime = Math.random() * 100, mistLast = 0;
function drawMist() {
  mctx.clearRect(0, 0, MS, MS);
  const c = MS / 2, R = MS * 0.3, steps = 96;
  mctx.beginPath();
  for (let k = 0; k <= steps; k++) {
    const a = k / steps * Math.PI * 2;
    let r = 1;
    for (const [n, depth, speed] of HARMONICS) r += depth * Math.sin(n * a + mistTime * speed * 6 + n * 1.3);
    r *= R * (1 + 0.04 * Math.sin(mistTime * 0.9)); // the whole sphere breathes a little
    const x = c + Math.cos(a) * r, y = c + Math.sin(a) * r;
    k ? mctx.lineTo(x, y) : mctx.moveTo(x, y);
  }
  mctx.closePath();
  const g = mctx.createRadialGradient(c, c, 0, c, c, R * 1.25);
  g.addColorStop(0, "rgba(255, 255, 255, .34)");
  g.addColorStop(0.6, "rgba(255, 255, 255, .2)");
  g.addColorStop(1, "rgba(255, 255, 255, .04)");
  mctx.fillStyle = g;
  mctx.fill();
}
(function mistLoop(t) {
  const playing = stage.classList.contains("audio") && stage.classList.contains("playing");
  if (playing && t - mistLast > 33) {
    mistTime += Math.min(t - mistLast, 100) / 1000 * 0.35;
    mistLast = t;
    drawMist();
  } else if (!playing) mistLast = t;
  requestAnimationFrame(mistLoop);
})(0);
drawMist();

// --- Matrix dot loader (same formula as transitions.dev's demo, recreated)
const dots = Array.from({ length: 25 }, () => $("loader").appendChild(document.createElement("i")));
let loadingSince = 0, loaderRaf = 0;
function dotLevel(t, i, j) {
  const dx = Math.abs(i - 2), dy = Math.abs(j - 2);
  const d = [Math.max(dx, dy), dx + dy, (i + j) / 2, Math.hypot(dx, dy)][Math.floor(t / 0.9) % 4];
  const wave = (Math.sin(t * 7 - d * 1.6) + 1) / 2;
  return 0.12 + 0.88 * wave * wave;
}
function loaderFrame(now) {
  const t = (now - loadingSince) / 1000;
  $("loader").classList.toggle("show", t > 0.18); // fast opens never flash it
  dots.forEach((el, k) => el.style.opacity = dotLevel(t, k % 5, Math.floor(k / 5)).toFixed(3));
  loaderRaf = requestAnimationFrame(loaderFrame);
}
function startLoading() { if (!loaderRaf) { loadingSince = performance.now(); loaderRaf = requestAnimationFrame(loaderFrame); } }
function stopLoading() { cancelAnimationFrame(loaderRaf); loaderRaf = 0; $("loader").classList.remove("show"); }

// --- Transport
const togglePlay = () => cmd("cycle", "pause");
$("play").onclick = togglePlay;
$("prev").onclick = () => cmd("playlist-prev");
$("next").onclick = () => cmd("playlist-next");
$("stepBack").onclick = () => cmd("frame-back-step");
$("stepFwd").onclick = () => cmd("frame-step");
$("fs").onclick = () => send({ t: "win", a: "fs" });

// Click on the picture: play / pause; double-click: fullscreen (like YouTube).
let clickTimer;
stage.addEventListener("click", ev => {
  if (ev.target.closest(".controls, .menu") || stage.classList.contains("idle")) return;
  clearTimeout(clickTimer); clickTimer = setTimeout(togglePlay, 220);
});
stage.addEventListener("dblclick", ev => {
  if (ev.target.closest(".controls, .menu")) return;
  clearTimeout(clickTimer); $("fs").onclick();
});

// Scrubbing: press and drag on the timeline, the picture follows in real time.
// Shift held: the position snaps to the nearest marker (within 40 px).
const SNAP_PX = 40, NEAR_PX = 8;
let scrubTarget = null, scrubRaf = 0, lastPointerX = null;
function pointAt(clientX, shift) {
  const r = $("timeline").getBoundingClientRect(), d = num("duration");
  let x = Math.min(r.width, Math.max(0, clientX - r.left));
  let t = r.width ? x / r.width * d : 0, mark = null, best = Infinity;
  if (d > 0) for (const c of json("chapter-list") || []) {
    const dist = Math.abs(c.time / d * r.width - x);
    if (dist < best) { best = dist; mark = c; }
  }
  const snapped = mark && shift && best <= SNAP_PX;
  if (snapped) { t = mark.time; x = mark.time / d * r.width; }
  return { t, x, width: r.width, mark: snapped || (mark && best <= NEAR_PX) ? mark : null };
}
function showHover(p) {
  const ht = $("hoverTime");
  ht.style.left = Math.min(p.width - 24, Math.max(24, p.x)) + "px";
  $("htTc").textContent = fmt(p.t);
  if (p.mark) $("htMk").textContent = p.mark.title || "Marqueur";
  ht.classList.toggle("on-mark", !!p.mark);
  ht.style.width = (p.mark ? $("htMk") : $("htTc")).offsetWidth + "px";
  document.querySelectorAll("#marks .mark").forEach(m => m.classList.toggle("active", !!p.mark && +m.dataset.time === p.mark.time));
}
function scrubTo(p) {
  const d = num("duration");
  $("fill").style.width = (d ? p.t / d * 100 : 0) + "%";
  scrubTarget = p.t;
  if (!scrubRaf) scrubRaf = requestAnimationFrame(() => { scrubRaf = 0; if (scrubTarget != null) cmd("seek", scrubTarget, "absolute+exact"); });
}
function onTimeline(clientX, shift) {
  lastPointerX = clientX;
  const p = pointAt(clientX, shift);
  showHover(p);
  if (scrubbing) scrubTo(p);
}
$("timeline").addEventListener("pointerdown", ev => {
  if (ev.button !== 0) return;
  $("timeline").setPointerCapture(ev.pointerId);
  scrubbing = true; $("timeline").classList.add("scrubbing"); onTimeline(ev.clientX, ev.shiftKey);
});
$("timeline").addEventListener("pointermove", ev => onTimeline(ev.clientX, ev.shiftKey));
$("timeline").addEventListener("pointerleave", () => { if (!scrubbing) lastPointerX = null; });
$("timeline").addEventListener("pointerup", () => { scrubbing = false; $("timeline").classList.remove("scrubbing"); });
// Pressing or releasing Shift without moving the mouse updates the snap at once.
["keydown", "keyup"].forEach(type => addEventListener(type, ev => {
  if (ev.key === "Shift" && lastPointerX != null) onTimeline(lastPointerX, type === "keydown");
}));
new ResizeObserver(([e]) => $("fill").style.setProperty("--full", e.contentRect.width + "px")).observe($("timeline"));

// --- Volume: target set instantly, mpv's volume glides there (no clicks or jumps).
let volTarget = null, volCurrent = null, volRaf = 0;
function renderVolume() {
  const muted = st.mute === "yes";
  const v = muted ? 0 : (volTarget ?? num("volume") / 100);
  $("level").style.width = v * 100 + "%"; $("knob").style.left = v * 100 + "%";
  swapIcon($("volIcon"), v === 0 ? "volume-off" : v < 0.5 ? "volume-1" : "volume-2");
}
function setVolume(v) {
  volTarget = Math.min(1, Math.max(0, v));
  if (st.mute === "yes") cmd("set", "mute", "no");
  if (volCurrent == null) volCurrent = num("volume") / 100;
  if (!volRaf) volRaf = requestAnimationFrame(glide);
  renderVolume(); flashVol();
}
function glide() {
  volCurrent += (volTarget - volCurrent) * 0.25;
  if (Math.abs(volTarget - volCurrent) < 0.004) volCurrent = volTarget;
  cmd("set", "volume", (volCurrent * 100).toFixed(1));
  if (volCurrent !== volTarget) volRaf = requestAnimationFrame(glide);
  else { volRaf = 0; volTarget = null; volCurrent = null; }
}
let volTimer;
const flashVol = () => { $("vol").classList.add("active"); clearTimeout(volTimer); volTimer = setTimeout(() => $("vol").classList.remove("active"), 1200); };
$("mute").onclick = () => cmd("cycle", "mute");
$("vol").addEventListener("wheel", ev => { ev.preventDefault(); setVolume((volTarget ?? num("volume") / 100) - Math.sign(ev.deltaY) * 0.05); }, { passive: false });
function railSet(ev) { const r = $("rail").getBoundingClientRect(); setVolume((ev.clientX - r.left) / r.width); }
$("rail").addEventListener("pointerdown", ev => {
  $("rail").setPointerCapture(ev.pointerId); $("rail").classList.add("dragging");
  railSet(ev); $("rail").onpointermove = railSet;
});
$("rail").addEventListener("pointerup", () => { $("rail").onpointermove = null; $("rail").classList.remove("dragging"); });

// --- Alpha: checkerboard behind transparent videos (mpv draws it)
$("alphaBtn").onclick = () => {
  const on = st.background !== "tiles";
  setProp("background", on ? "tiles" : "color");
  toast(on ? "Damier de transparence" : "Fond uni");
};

// --- Menus: open from where they were invoked (transform-origin follows the anchor).
const menus = [$("menu"), $("subsMenu"), $("setMenu")];
const anyMenuOpen = () => menus.some(m => m.classList.contains("open"));
const closeAll = () => { menus.forEach(m => m.classList.remove("open")); $("subsBtn").classList.remove("on"); $("setBtn").classList.remove("on"); };
function openAt(menu, x, y, origin) {
  closeAll();
  const w = menu.offsetWidth, h = menu.offsetHeight;
  const left = Math.max(8, Math.min(x, innerWidth - w - 8)), top = Math.max(8, Math.min(y, innerHeight - h - 8));
  menu.style.left = left + "px"; menu.style.top = top + "px";
  menu.style.transformOrigin = origin ?? `${x - left}px ${y - top}px`;
  menu.classList.add("open");
}
function popover(btn, menu, build) {
  btn.onclick = ev => {
    ev.stopPropagation();
    if (menu.classList.contains("open")) return closeAll();
    build();
    const b = btn.getBoundingClientRect();
    openAt(menu, b.right - menu.offsetWidth, b.top - menu.offsetHeight - 8, "bottom right");
    btn.classList.add("on"); hideTip();
  };
}
const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
const trackName = tr => [tr.title, tr.lang && tr.lang.toUpperCase()].filter(Boolean).join(" · ") || `Piste ${tr.id}`;

function buildSubs() {
  const m = $("subsMenu"), tracks = (json("track-list") || []).filter(t => t.type === "sub");
  const sid = st.sid;
  const items = [el("div", "head", "Sous-titres")];
  const off = el("div", "item" + (sid === "no" || !tracks.some(t => t.selected) ? " checked" : ""), "Désactivés");
  off.onclick = () => { cmd("set", "sid", "no"); closeAll(); };
  items.push(off);
  for (const tr of tracks) {
    const it = el("div", "item" + (tr.selected ? " checked" : ""), trackName(tr));
    it.onclick = () => { cmd("set", "sid", tr.id); closeAll(); toast("Sous-titres : " + trackName(tr)); };
    items.push(it);
  }
  const load = el("div", "item", "Charger un fichier…");
  load.onclick = () => { closeAll(); send({ t: "open-subs" }); };
  items.push(el("div", "sep"), load);
  m.replaceChildren(...items);
}
function buildSettings() {
  const m = $("setMenu");
  const speeds = el("div", "chips");
  for (const s of [0.5, 0.75, 1, 1.5, 2]) {
    const chip = el("span", Math.abs(num("speed") - s) < 0.01 ? "checked" : "", `${s}×`);
    chip.onclick = () => { cmd("set", "speed", s); speeds.querySelectorAll("span").forEach(c => c.classList.toggle("checked", c === chip)); };
    speeds.append(chip);
  }
  const loop = el("div", "item" + (st["loop-file"] === "inf" ? " checked" : ""), "Lire en boucle");
  loop.onclick = () => { cmd("set", "loop-file", st["loop-file"] === "inf" ? "no" : "inf"); closeAll(); };
  // keep-open=yes moves on to the next file of the folder; "always" stops at the end of each file.
  const chain = el("div", "item" + (st["keep-open"] !== "always" ? " checked" : ""), "Enchaîner le dossier");
  chain.onclick = () => { cmd("set", "keep-open", st["keep-open"] === "always" ? "yes" : "always"); closeAll(); };
  const items = [el("div", "head", "Vitesse"), speeds, el("div", "sep"), loop, chain];
  const audios = (json("track-list") || []).filter(t => t.type === "audio");
  if (audios.length > 1) {
    items.push(el("div", "sep"), el("div", "head", "Piste audio"));
    for (const tr of audios) {
      const it = el("div", "item" + (tr.selected ? " checked" : ""), trackName(tr));
      it.onclick = () => { cmd("set", "aid", tr.id); closeAll(); };
      items.push(it);
    }
  }
  const hw = st["hwdec-current"];
  const dec = el("div", "item");
  dec.append(el("span", "", "Décodage"), el("span", "val", hw && hw !== "no" ? `GPU (${hw})` : "CPU"));
  items.push(el("div", "sep"), dec);
  m.replaceChildren(...items);
}
popover($("subsBtn"), $("subsMenu"), buildSubs);
popover($("setBtn"), $("setMenu"), buildSettings);
stage.addEventListener("contextmenu", ev => { ev.preventDefault(); openAt($("menu"), ev.clientX, ev.clientY); });
addEventListener("pointerdown", ev => { if (!menus.some(m => m.contains(ev.target)) && !ev.target.closest("#subsBtn, #setBtn")) closeAll(); });

let toastTimer;
function toast(msg) {
  $("toast").textContent = msg; $("toast").classList.add("show");
  clearTimeout(toastTimer); toastTimer = setTimeout(() => $("toast").classList.remove("show"), 1800);
}

// Editor: B / N mark the selection (mpv's A-B loop previews it), Ctrl+E exports it.
const markIn = () => { cmd("set", "ab-loop-a", num("time-pos")); toast("Début de l'extrait : " + fmt(num("time-pos"))); };
const markOut = () => { cmd("set", "ab-loop-b", num("time-pos")); toast("Fin de l'extrait : " + fmt(num("time-pos"))); };
const clearSel = () => { cmd("set", "ab-loop-a", "no"); cmd("set", "ab-loop-b", "no"); toast("Sélection effacée"); };
const actions = {
  save: () => send({ t: "save-frame" }), copy: () => send({ t: "copy-frame" }), export: () => send({ t: "export" }),
  markIn, markOut, clearSel, open: () => send({ t: "open" }), folder: () => send({ t: "show-in-folder" }),
  play: togglePlay, fs: () => $("fs").onclick(), quit: () => send({ t: "win", a: "close" }),
};
$("menu").addEventListener("click", ev => {
  const act = ev.target.closest(".item")?.dataset.act; if (!act) return;
  closeAll(); actions[act]?.();
});

// --- Tooltips: 500 ms delay the first time, then the next ones pop right away; gone at once on leave.
let tipTimer, tipWarm = false, coolTimer;
function showTip(btn) {
  const b = btn.getBoundingClientRect(), tip = $("tip");
  tip.classList.remove("show"); void tip.offsetWidth; // restart the pop for every button
  tip.textContent = btn.dataset.tip;
  tip.style.left = Math.max(40, Math.min(b.left + b.width / 2, innerWidth - 60)) + "px";
  tip.style.top = (b.top - 34) + "px";
  tip.classList.add("show"); tipWarm = true; clearTimeout(coolTimer);
}
function hideTip() {
  clearTimeout(tipTimer); $("tip").classList.remove("show");
  coolTimer = setTimeout(() => tipWarm = false, 400);
}
document.querySelectorAll(".bar [data-tip]").forEach(btn => {
  btn.addEventListener("mouseenter", () => {
    if (anyMenuOpen()) return;
    clearTimeout(tipTimer);
    tipWarm ? showTip(btn) : tipTimer = setTimeout(() => showTip(btn), 500);
  });
  btn.addEventListener("mouseleave", hideTip);
  btn.addEventListener("click", hideTip);
});

// --- Window: dedicated title bar drags the window, buttons, resize grips
// Move / resize: the page keeps the pointer and sends screen positions (physical px) to Rust.
let grabbing = null, grabRaf = 0, grabXY = null;
const screenXY = ev => ({ x: ev.screenX * devicePixelRatio, y: ev.screenY * devicePixelRatio });
function startGrab(ev, dir) {
  ev.currentTarget.setPointerCapture(ev.pointerId);
  grabbing = ev.currentTarget;
  send({ t: "win", a: "grab", dir, ...screenXY(ev) });
}
function moveGrab(ev) {
  if (!grabbing) return;
  grabXY = screenXY(ev);
  if (!grabRaf) grabRaf = requestAnimationFrame(() => { grabRaf = 0; send({ t: "win", a: "grab-move", ...grabXY }); });
}
function endGrab() { if (grabbing) { grabbing = null; send({ t: "win", a: "grab-end" }); } }
$("titlebar").addEventListener("pointerdown", ev => {
  // (pointerdown has no click count: detail is always 0, so no filter on it)
  if (ev.button === 0 && !ev.target.closest(".wbtn")) startGrab(ev, "");
});
$("titlebar").addEventListener("pointermove", moveGrab);
$("titlebar").addEventListener("pointerup", endGrab);
$("titlebar").addEventListener("lostpointercapture", endGrab);
$("titlebar").addEventListener("dblclick", ev => { if (!ev.target.closest(".wbtn")) send({ t: "win", a: "max" }); });
$("wMin").onclick = () => send({ t: "win", a: "min" });
$("wMax").onclick = () => send({ t: "win", a: "max" });
$("wClose").onclick = () => send({ t: "win", a: "close" });
document.querySelectorAll(".grip").forEach(g => {
  g.addEventListener("pointerdown", ev => { if (ev.button === 0) startGrab(ev, g.dataset.dir); });
  g.addEventListener("pointermove", moveGrab);
  g.addEventListener("pointerup", endGrab);
  g.addEventListener("lostpointercapture", endGrab);
});

// --- Keyboard
addEventListener("keydown", ev => {
  const k = ev.key, c = ev.ctrlKey;
  if (k === "Escape") { if (anyMenuOpen()) closeAll(); else if (win.classList.contains("fullscreen")) $("fs").onclick(); return; }
  if (c && k.toLowerCase() === "o") { ev.preventDefault(); actions.open(); return; }
  if (c && k.toLowerCase() === "q") { ev.preventDefault(); actions.quit(); return; }
  if (stage.classList.contains("idle")) return;
  if (k === " ") { ev.preventDefault(); togglePlay(); }
  else if (k === "ArrowRight") cmd("seek", 5, "relative+exact");
  else if (k === "ArrowLeft") cmd("seek", -5, "relative+exact");
  else if (k === "ArrowUp") { ev.preventDefault(); setVolume((volTarget ?? num("volume") / 100) + 0.05); }
  else if (k === "ArrowDown") { ev.preventDefault(); setVolume((volTarget ?? num("volume") / 100) - 0.05); }
  else if (k === ".") cmd("frame-step");
  else if (k === ",") cmd("frame-back-step");
  else if (k === "PageDown") cmd("playlist-next");
  else if (k === "PageUp") cmd("playlist-prev");
  else if (c && k.toLowerCase() === "s") { ev.preventDefault(); actions.save(); }
  else if (c && k.toLowerCase() === "c") actions.copy();
  else if (c && k.toLowerCase() === "e") { ev.preventDefault(); actions.export(); }
  else if (ev.altKey && k.toLowerCase() === "x") clearSel();
  else if (c) return;
  else if (k === "m") cmd("cycle", "mute");
  else if (k === "f") $("fs").onclick();
  else if (k === "t" && !$("alphaBtn").hidden) $("alphaBtn").onclick();
  else if (k === "l") { cmd("set", "loop-file", st["loop-file"] === "inf" ? "no" : "inf"); toast(st["loop-file"] === "inf" ? "Boucle désactivée" : "Lecture en boucle"); }
  else if (k === "s") $("subsBtn").click();
  else if (k === "b") markIn();
  else if (k === "n") markOut();
});

// Controls and cursor hide after 3 s without movement, or as soon as the mouse leaves the window
// (never while a menu is open).
let hideTimer;
const hideUi = () => { if (!anyMenuOpen() && !scrubbing) { stage.classList.add("hide-ui"); hideTip(); } };
addEventListener("mousemove", () => {
  stage.classList.remove("hide-ui"); clearTimeout(hideTimer);
  hideTimer = setTimeout(hideUi, 3000);
});
document.documentElement.addEventListener("mouseleave", () => { clearTimeout(hideTimer); hideUi(); });

// Update prompt buttons. "Ne plus demander" is remembered by the Rust side (settings.json).
let canInstall = false;
function closeUpdate(download) {
  send({ t: "update", download, never: $("updateNever").checked });
  $("updateBack").classList.remove("open");
}
$("updateGo").onclick = () => {
  if (!canInstall) return closeUpdate(true);
  send({ t: "install-update", never: $("updateNever").checked });
  vb.updateProgress(0);
};
// The discreet button: shows the prompt again if an update is known, otherwise checks now.
$("updBtn").onclick = () => {
  if ($("updBtn").classList.contains("available")) { $("updateNever").checked = false; $("updateBack").classList.add("open"); }
  else { $("updText").textContent = "Recherche…"; send({ t: "check-update" }); }
};
$("updateLater").onclick = () => closeUpdate(false);

send({ t: "ready" });
