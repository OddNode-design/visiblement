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
  toast: msg => /^Frame (copiée|enregistrée)/.test(msg) ? notice(msg) : toast(msg),
  drag(on, x, y) { stage.classList.toggle("dragging", on); plDropMark(on ? plDropIndex(x, y) : null); },
  // Startup check found a newer GitHub release (src/main.rs check_for_update).
  // prompt false: "Ne plus demander" was ticked, so only the discreet button lights up.
  updateAvailable({ latest, current, prompt, installable }) {
    const v = latest.replace(/^v/, "");
    canInstall = !!installable;
    $("updateGo").textContent = canInstall ? "Installer" : "Télécharger";
    $("updateText").innerHTML = `Visiblement <b>${v}</b> est prête. Tu as la version ${current}.`;
    $("updBtn").classList.add("available");
    $("verText").hidden = true;
    $("updBtn").setAttribute("aria-label", `Mettre à jour vers la version ${v}`);
    if (prompt) { $("updateNever").checked = false; $("updateBack").classList.add("open"); }
  },
  // Download progress (0-100), then "install" while Windows Installer takes over.
  updateProgress(p) {
    $("updateBack").querySelector(".modal").classList.add("installing");
    const bar = $("updateBar");
    if (p === "install") { bar.classList.add("busy"); $("updateLabel").textContent = "Installation… Visiblement va redémarrer"; }
    else { bar.classList.remove("busy"); bar.style.width = p + "%"; $("updateLabel").textContent = `Téléchargement… ${p} %`; }
  },
  updateFailed() {
    $("updateBack").querySelector(".modal").classList.remove("installing");
    canInstall = false; $("updateGo").textContent = "Télécharger";
    $("updateBack").classList.add("open");
    toast("Échec de la mise à jour : télécharge-la depuis la page des versions");
  },
  // No update: just the installed version, small, where the update button would be.
  version(v) { $("verText").textContent = `Version ${v}`; },
  maximized(on) { win.classList.toggle("maximized", on); swapIcon($("wMax").querySelector(".ico"), on ? "copy" : "square"); },
  fullscreen(on) { win.classList.toggle("fullscreen", on); win.classList.remove("peek"); swapIcon($("fsIcon"), on ? "minimize" : "maximize"); },
};

function render() {
  const idle = st["idle-active"] === "yes";
  const audio = !idle && !json("current-tracks/video") && !!json("current-tracks/audio");
  stage.classList.toggle("idle", idle);
  stage.classList.toggle("audio", audio);

  const t = num("time-pos"), d = num("duration");
  // Streams: the real title (e.g. the YouTube video name) rather than the end of the address.
  const isStream = (st.path || "").includes("://");
  const name = idle ? "Visiblement" : ((isStream && st["media-title"]) || st.filename || "Visiblement");
  if ($("title").textContent !== name) $("title").textContent = name;
  const vp = json("video-params");
  $("res").textContent = !idle && !audio && vp && vp.w ? `${vp.w} × ${vp.h}` : "";
  // 360° view on by itself: spatial-media metadata in the file (vb.spherical), or a 2:1 picture named "360".
  if (vp && vp.w && `${st.path}|${vp.w}x${vp.h}` !== v360Key) {
    v360Key = `${st.path}|${vp.w}x${vp.h}`;
    const named = vp.w === 2 * vp.h && /360/.test(st.filename || "");
    if (named !== v360) set360(named, !named);
    if (!isStream) send({ t: "probe360", path: st.path });
  }
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
  // At the end of the last media (mpv holds its last frame): Rejouer instead of Lecture.
  const ended = st["eof-reached"] === "yes" && !idle;
  swapIcon($("playIcon"), ended ? "rotate-ccw" : paused ? "play" : "pause");
  $("play").dataset.tip = ended ? "Rejouer (Espace)" : paused ? "Lecture (Espace)" : "Pause (Espace)";
  renderPlaylist();

  renderVolume();
  renderMarks(d);
  layoutMask();

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
  wakeEyes();
});
// The eye loop sleeps while nothing moves (no frame drawn at all) and wakes for the next blink,
// glance or mouse move.
let eyeSleep = 0, eyeAwake = true;
function wakeEyes() {
  if (eyeAwake) return;
  clearTimeout(eyeSleep); eyeAwake = true; requestAnimationFrame(eyeLoop);
}
// Organic bar heights: a few slow sines per bar, never in sync.
const waveHeight = (i, t) => {
  const s = t / 1000;
  const v = Math.sin(s * 3.1 + i * 1.7) * 0.5 + Math.sin(s * 5.3 + i * 0.9) * 0.3 + Math.sin(s * 1.3 + i * 2.3) * 0.2;
  const envelope = 1 - Math.abs(i - (BARS - 1) / 2) / BARS; // taller in the middle
  return 18 + (v * 0.5 + 0.5) * 70 * envelope;
};
function eyeLoop(t) {
  let moved = false;
  const shown = stage.classList.contains("idle") || stage.classList.contains("audio");
  if (shown) {
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
      const attrs = [(b.x - b.w / 2).toFixed(1), (b.y - b.h / 2).toFixed(1), Math.max(0, b.w).toFixed(1), Math.max(0, b.h).toFixed(1), Math.max(0, r).toFixed(1), b.o.toFixed(2)];
      const key = attrs.join();
      if (key !== b.last) {
        b.last = key; moved = true;
        ["x", "y", "width", "height", "rx", "opacity"].forEach((name, n) => b.el.setAttribute(name, attrs[n]));
      }
    });
  }
  const wave = shown && stage.classList.contains("audio") && stage.classList.contains("playing");
  if (moved || wave) { requestAnimationFrame(eyeLoop); return; }
  // Still: sleep until the next blink or glance (or a check every 0.5 s while the mascot is hidden).
  const next = Math.min(nextBlink, Math.max(nextGlance, lastMove + 2500));
  eyeAwake = false;
  eyeSleep = setTimeout(() => { eyeAwake = true; requestAnimationFrame(eyeLoop); }, shown ? Math.max(16, next - t) : 500);
}
requestAnimationFrame(eyeLoop);

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
  if (playing) requestAnimationFrame(mistLoop);
  else setTimeout(() => requestAnimationFrame(mistLoop), 300); // no audio playing: no frames
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
const togglePlay = () => {
  if (st["eof-reached"] === "yes") { cmd("seek", 0, "absolute"); cmd("set", "pause", "no"); } // Rejouer
  else cmd("cycle", "pause");
};
$("play").onclick = togglePlay;
$("prev").onclick = () => cmd("playlist-prev");
$("next").onclick = () => cmd("playlist-next");
$("stepBack").onclick = () => cmd("frame-back-step");
$("stepFwd").onclick = () => cmd("frame-step");
$("fs").onclick = () => send({ t: "win", a: "fs" });

// Click on the picture: play / pause (no double-click fullscreen: F or the button do it).
stage.addEventListener("click", ev => {
  if (ev.target.closest(".controls, .menu") || stage.classList.contains("idle")) return;
  if (looked) { looked = false; return; } // end of a 360° drag, not a click
  togglePlay();
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

function buildSubs() { $("subsMenu").replaceChildren(...subsItems()); }
// The subtitle choices: in their own menu, or inside the settings when the bar is too narrow.
function subsItems() {
  const tracks = (json("track-list") || []).filter(t => t.type === "sub");
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
  return items;
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
  const items = [el("div", "head", "Vitesse"), speeds, el("div", "sep"), loop];
  // Narrow bar: the buttons it hides live here (like the title bar's burger).
  if (document.querySelector(".bar").classList.contains("compact")) {
    const list = el("div", "item" + ($("playlist").classList.contains("open") ? " checked" : ""), "Liste de lecture");
    list.onclick = () => { closeAll(); togglePlaylist(); };
    items.push(list);
    if (!$("alphaBtn").hidden) {
      const tiles = el("div", "item" + (st.background === "tiles" ? " checked" : ""), "Damier de transparence");
      tiles.onclick = () => { closeAll(); $("alphaBtn").onclick(); };
      items.push(tiles);
    }
    items.push(el("div", "sep"), ...subsItems());
  }
  const audios = (json("track-list") || []).filter(t => t.type === "audio");
  if (audios.length > 1) {
    items.push(el("div", "sep"), el("div", "head", "Piste audio"));
    for (const tr of audios) {
      const it = el("div", "item" + (tr.selected ? " checked" : ""), trackName(tr));
      it.onclick = () => { cmd("set", "aid", tr.id); closeAll(); };
      items.push(it);
    }
  }
  const langs = el("div", "chips");
  for (const [code, label] of [["fr", "Français"], ["en", "English"]]) {
    const chip = el("span", i18n.lang === code ? "checked" : "", label);
    chip.onclick = () => { setLang(code); buildSettings(); };
    langs.append(chip);
  }
  items.push(el("div", "sep"), el("div", "head", "Langue"), langs);
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
// Export / conversion running (Rust): a pill with its progress and a stop button.
vb.job = (label, pct) => {
  $("jobText").textContent = label; $("jobPct").textContent = `${pct} %`;
  $("job").classList.add("show"); $("toast").classList.remove("show");
};
vb.jobDone = () => $("job").classList.remove("show");
$("jobStop").onclick = () => { $("jobText").textContent = "Arrêt…"; send({ t: "cancel-encode" }); };
// Confirmation of a copied or saved frame, at the top: the menu that asked for it is gone by then.
let noticeTimer;
function notice(msg) {
  $("noticeText").textContent = msg;
  const n = $("notice");
  n.classList.remove("show"); void n.offsetWidth; // replay the drop for a second copy in a row
  n.classList.add("show");
  clearTimeout(noticeTimer); noticeTimer = setTimeout(() => n.classList.remove("show"), 2000);
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
  if (ev.button === 0 && !ev.target.closest(".wbtn, .nav, .burger, .home")) startGrab(ev, "");
});
$("titlebar").addEventListener("pointermove", moveGrab);
$("titlebar").addEventListener("pointerup", endGrab);
$("titlebar").addEventListener("lostpointercapture", endGrab);
$("titlebar").addEventListener("dblclick", ev => { if (!ev.target.closest(".wbtn, .nav, .burger, .home")) send({ t: "win", a: "max" }); });
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
  if (k === "Escape") {
    const open = document.querySelector(".modal-back.open:not(#updateBack)");
    if (open) closeModal(open); else if (closeNav()) {} else if (anyMenuOpen()) closeAll(); else if (win.classList.contains("fullscreen")) $("fs").onclick();
    return;
  }
  if (document.querySelector(".modal-back.open")) { if (k === "Enter" && $("netBack").classList.contains("open")) $("netGo").click(); return; }
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
  else if (c && k.toLowerCase() === "m") { ev.preventDefault(); actions.toggleMask(); }
  else if (c && k.toLowerCase() === "i") { ev.preventDefault(); actions.invertMask(); }
  else if (ev.altKey && k.toLowerCase() === "x") clearSel();
  else if (c) return;
  else if (k === "m") cmd("cycle", "mute");
  else if (k === "f") $("fs").onclick();
  else if (k === "t" && !$("alphaBtn").hidden) $("alphaBtn").onclick();
  else if (k === "l") { cmd("set", "loop-file", st["loop-file"] === "inf" ? "no" : "inf"); toast(st["loop-file"] === "inf" ? "Boucle désactivée" : "Lecture en boucle"); }
  else if (k === "s") $("subsBtn").click();
  else if (k === "v") actions.toggle360();
  else if (k === "p") togglePlaylist();
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
// The round button (shown only when an update exists): the prompt again.
$("updBtn").onclick = () => { $("updateNever").checked = false; $("updateBack").classList.add("open"); };
// The dive starts on hover and always plays to the end (leaving early no longer cuts it).
$("updBtn").addEventListener("mouseenter", () => $("updBtn").classList.add("diving"));
$("updBtn").addEventListener("animationend", () => $("updBtn").classList.remove("diving"));
$("updateLater").onclick = () => closeUpdate(false);

// --- Navigation bar (Média / Éditer / Infos): menus open on hover, or on click
const navItems = [...document.querySelectorAll(".nav-item")];
let navTimer;
function openNav(item) { navItems.forEach(i => i.classList.toggle("open", i === item && !!item.querySelector(".dropdown"))); closeAll(); hideTip(); }
function closeNav() {
  const was = navItems.some(i => i.classList.contains("open")) || $("titlebar").classList.contains("menu-open");
  navItems.forEach(i => i.classList.remove("open"));
  setBurger(false);
  return was;
}
function setBurger(open) { $("titlebar").classList.toggle("menu-open", open); $("burger").setAttribute("aria-expanded", open); }
$("burger").onclick = () => { const open = !$("titlebar").classList.contains("menu-open"); closeNav(); setBurger(open); };

// Responsive title bar: the centred navigation needs room on both sides of it. When the window is
// too narrow, it folds into the burger; the title takes what is left, then the resolution goes.
function fitTitleBar() {
  const bar = $("titlebar");
  // Measured laid out in a row (not as the burger panel); dropdowns are left out of the width.
  bar.classList.remove("compact");
  const navW = navItems.reduce((w, i) => w + i.offsetWidth, 6 * (navItems.length - 1)), barW = bar.clientWidth;
  const right = [...bar.querySelectorAll(".pos, .wbtn")].reduce((w, e) => w + e.offsetWidth + 10, 4);
  const left = 12 + 22 + 10; // padding, logo, gap
  const compact = barW / 2 - navW / 2 < Math.max(left + 90, right) + 12;
  bar.classList.toggle("compact", compact);
  if (!compact) setBurger(false);
  const resW = $("res").textContent ? $("res").offsetWidth + 10 : 0;
  const room = compact ? barW - right - left - 42 - 12 : barW / 2 - navW / 2 - left - 16;
  bar.classList.toggle("no-res", room - resW < 90);
  bar.style.setProperty("--title-max", Math.max(40, room - (room - resW < 90 ? 0 : resW)) + "px");
}
new ResizeObserver(fitTitleBar).observe($("titlebar"));
new MutationObserver(fitTitleBar).observe($("pos"), { childList: true, characterData: true, subtree: true });
new MutationObserver(fitTitleBar).observe($("res"), { childList: true, characterData: true, subtree: true });
navItems.forEach(item => {
  item.addEventListener("mouseenter", () => { clearTimeout(navTimer); navTimer = setTimeout(() => openNav(item), navItems.some(i => i.classList.contains("open")) ? 0 : 90); });
  // Leaving an item closes its menu only: the burger panel stays open.
  item.addEventListener("mouseleave", () => { clearTimeout(navTimer); navTimer = setTimeout(() => navItems.forEach(i => i.classList.remove("open")), 220); });
  item.querySelector(".nav-btn").addEventListener("click", () => { clearTimeout(navTimer); openNav(item); });
});
addEventListener("pointerdown", ev => { if (!ev.target.closest(".nav, .burger")) closeNav(); });

const modalOpen = id => { closeNav(); closeAll(); $(id).classList.add("open"); };
function closeModal(back) { back.classList.remove("open"); if (back.id === "infoBack") clearInterval(infoTimer); }
document.querySelectorAll(".modal-back:not(#updateBack)").forEach(back => {
  back.addEventListener("pointerdown", ev => { if (ev.target === back) closeModal(back); });
  back.querySelectorAll("[data-close]").forEach(b => b.onclick = () => closeModal(back));
});

Object.assign(actions, {
  openFiles: () => send({ t: "open-files" }),
  openFolder: () => send({ t: "open-folder" }),
  openUrl: () => { modalOpen("netBack"); setTimeout(() => $("netUrl").focus(), 50); },
  openCamera: () => {
    $("camText").textContent = "Recherche des caméras…"; $("camList").replaceChildren();
    $("ndiText").textContent = "Recherche sur le réseau…"; $("ndiList").replaceChildren();
    modalOpen("camBack"); send({ t: "list-cameras" }); send({ t: "list-ndi" });
  },
});
document.querySelectorAll(".dropdown .dd[data-act]").forEach(b => b.addEventListener("click", () => { closeNav(); actions[b.dataset.act]?.(); }));
$("netExamples").querySelectorAll("button").forEach(b => b.onclick = () => {
  const v = b.textContent.replace("…", ""); $("netUrl").value = v; $("netUrl").focus();
  $("netUrl").setSelectionRange(v.length, v.length);
});
$("netGo").onclick = () => { const url = $("netUrl").value.trim(); if (!url) return; send({ t: "open-url", url }); closeModal($("netBack")); };

// Recent media (Rust keeps the list in settings.json and sends it here)
vb.recent = list => {
  const items = (list || []).map(path => {
    const name = path.split(/[\\/]/).pop(), dir = path.slice(0, path.length - name.length);
    const b = el("button", "dd");
    const icon = el("i", "ico"); icon.dataset.i = "file"; setSrc(icon);
    const text = el("span", "dd-text"); text.append(el("b", "", name), el("small", "", dir));
    b.append(icon, text);
    b.onclick = () => { closeNav(); send({ t: "open-path", path }); };
    return b;
  });
  $("recentMenu").replaceChildren(...(items.length ? items : [el("div", "dd disabled", "Aucun média récent")]));
};

// NDI sources on the network (null: no NDI runtime on this PC)
vb.ndi = list => {
  $("ndiText").textContent = list === null ? "Installez NDI Tools pour recevoir les sources NDI."
    : list.length ? "Sources trouvées sur le réseau :" : "Aucune source NDI trouvée sur le réseau.";
  $("ndiList").replaceChildren(...(list || []).map(name => {
    const b = el("button");
    const icon = el("i", "ico"); icon.dataset.i = "globe"; setSrc(icon);
    b.append(icon, el("span", "", name));
    b.onclick = () => { send({ t: "open-ndi", name }); closeModal($("camBack")); };
    return b;
  }));
};

// Capture devices
vb.cameras = list => {
  $("camText").textContent = list.length
    ? "Choisissez une caméra, une carte d'acquisition ou une caméra virtuelle."
    : "Aucune webcam ni carte d'acquisition détectée.";
  $("camList").replaceChildren(...list.map(name => {
    const b = el("button");
    const icon = el("i", "ico"); icon.dataset.i = "webcam"; setSrc(icon);
    b.append(icon, el("span", "", name));
    b.onclick = () => { send({ t: "open-camera", name }); closeModal($("camBack")); };
    return b;
  }));
};

// --- Infos panel: Général, Métadonnées, Codec, Statistiques (statistics refresh every second)
let info = null, infoTab = "general", infoTimer;
const parse = v => { try { return JSON.parse(v ?? "null"); } catch { return null; } };
const nf = (v, d = 0) => Number(v).toLocaleString("fr-FR", { maximumFractionDigits: d, minimumFractionDigits: d });
const bytes = b => { b = +b; if (!b) return "–"; const u = ["o", "Ko", "Mo", "Go", "To"]; let i = 0; while (b >= 1024 && i < u.length - 1) { b /= 1024; i++; } return `${nf(b, i ? 1 : 0)} ${u[i]}`; };
const rate = bps => { bps = +bps; if (!bps) return "–"; return bps >= 1e6 ? `${nf(bps / 1e6, 1)} Mb/s` : `${nf(bps / 1e3, 0)} kb/s`; };
const date = ms => ms ? new Date(ms).toLocaleString("fr-FR", { dateStyle: "long", timeStyle: "short" }) : "–";
const clockLong = s => { s = +s; if (!Number.isFinite(s)) return "–"; const ms = Math.round((s % 1) * 1000); return `${fmt(s)}.${String(ms).padStart(3, "0")}`; };
function rows(pairs) {
  const g = el("div", "rows");
  for (const [k, v] of pairs) if (v !== undefined && v !== null && v !== "") g.append(el("div", "", k), el("div", "", String(v)));
  return g;
}
const TRACK = { video: "Vidéo", audio: "Audio", sub: "Sous-titres" };
// FFmpeg names containers by demuxer ("mov,mp4,m4a,3gp,3g2,mj2"): show something readable.
const FORMATS = [[/^mov,mp4/, "QuickTime / MP4"], [/^matroska/, "Matroska / WebM"], [/^avi/, "AVI"], [/^wav/, "WAV"],
  [/^mp3/, "MP3"], [/^flac/, "FLAC"], [/^gif/, "GIF"], [/^webp/, "WebP"], [/^mpegts/, "MPEG-TS"], [/^mxf/, "MXF"], [/^hls/, "HLS"]];
const formatName = f => f ? (FORMATS.find(([re]) => re.test(f))?.[1] ?? f.split(",")[0].toUpperCase()) : null;
function renderInfo() {
  if (!info) return;
  const d = info, body = [];
  const vp = parse(d["video-params"]) || {}, ap = parse(d["audio-params"]) || {};
  const dur = +d.duration, size = +d["file-size"];
  $("infoName").textContent = d.filename || "Aucun média";
  $("infoSub").textContent = vp.w ? `${vp.w} × ${vp.h}` : "";
  if (infoTab === "general") {
    const path = d.path || "", name = path.split(/[\\/]/).pop();
    body.push(rows([
      ["Nom", d.filename], ["Dossier", path.slice(0, path.length - name.length) || "–"], ["Taille", bytes(size)],
      ["Durée", dur ? clockLong(dur) : "–"], ["Format", formatName(d["file-format"])],
      ["Résolution", vp.w ? `${vp.w} × ${vp.h}` : null], ["Images/s", d["container-fps"] ? nf(d["container-fps"], 3) : null],
      ["Nombre d'images", d["estimated-frame-count"]], ["Débit moyen", size && dur ? rate(size * 8 / dur) : null],
      ["Créé le", date(d.created)], ["Modifié le", date(d.modified)],
    ]));
  } else if (infoTab === "meta") {
    const meta = parse(d.metadata) || {}, entries = Object.entries(meta);
    body.push(entries.length ? rows(entries) : el("div", "empty", "Ce fichier ne contient pas de métadonnées."));
  } else if (infoTab === "codec") {
    for (const t of parse(d["track-list"]) || []) {
      body.push(el("h3", "", `${TRACK[t.type] || t.type} ${t.id}${t.selected ? " · active" : ""}`));
      const common = [["Codec", t["codec-desc"] ? `${t.codec} (${t["codec-desc"]})` : t.codec], ["Langue", t.lang], ["Titre", t.title]];
      if (t.type === "video") body.push(rows([...common,
        ["Dimensions", t["demux-w"] ? `${t["demux-w"]} × ${t["demux-h"]}` : null], ["Images/s", t["demux-fps"] ? nf(t["demux-fps"], 3) : null],
        ...(t.selected ? [["Format de pixel", vp.pixelformat], ["Couleurs", [vp.colormatrix, vp.primaries, vp.gamma].filter(Boolean).join(" · ")],
          ["Transparence", vp.alpha && vp.alpha !== "auto" ? vp.alpha : null], ["Décodage", d["hwdec-current"] && d["hwdec-current"] !== "no" ? `GPU (${d["hwdec-current"]})` : "Processeur"]] : []),
      ]));
      else if (t.type === "audio") body.push(rows([...common,
        ["Fréquence", t["demux-samplerate"] ? `${nf(t["demux-samplerate"])} Hz` : null], ["Canaux", t["demux-channel-count"] || ap["channel-count"]],
        ...(t.selected ? [["Échantillons", ap.format]] : []),
      ]));
      else body.push(rows(common));
    }
    if (!body.length) body.push(el("div", "empty", "Aucune piste."));
  } else {
    body.push(rows([
      ["Débit vidéo", rate(d["video-bitrate"])], ["Débit audio", rate(d["audio-bitrate"])],
      ["Images/s affichées", d["estimated-vf-fps"] ? nf(d["estimated-vf-fps"], 2) : "–"], ["Fréquence de l'écran", d["display-fps"] ? `${nf(d["display-fps"], 2)} Hz` : "–"],
      ["Images perdues", `${d["frame-drop-count"] ?? 0} à l'affichage · ${d["decoder-frame-drop-count"] ?? 0} au décodage`],
      ["Écart audio / vidéo", d.avsync ? `${nf(d.avsync * 1000, 1)} ms` : "–"], ["Mémoire tampon", d["demuxer-cache-duration"] ? `${nf(d["demuxer-cache-duration"], 1)} s` : "–"],
      ["Vitesse", d.speed ? `${nf(d.speed, 2)}×` : "–"],
    ]));
  }
  $("infoBody").replaceChildren(...body);
}
// Slides a tab switcher's white pill under its selected button (Infos tabs, Convertir source).
function moveTabPill(box = $("infoTabs")) {
  const on = box.querySelector("button.on"), pill = box.querySelector(".pill");
  pill.style.width = on.offsetWidth + "px"; pill.style.translate = (on.offsetLeft - 3) + "px 0";
}
$("infoTabs").querySelectorAll("button").forEach(b => b.onclick = () => {
  $("infoTabs").querySelectorAll("button").forEach(x => x.classList.toggle("on", x === b));
  infoTab = b.dataset.tab; moveTabPill(); renderInfo();
  const body = $("infoBody"); body.scrollTop = 0; body.classList.remove("swap"); void body.offsetWidth; body.classList.add("swap");
});
vb.info = data => { info = data; renderInfo(); };
// --- Convertir: the open file, or an image sequence, into another format (src/editor.rs encodes;
// the ids "codec:type" match editor::PRESETS). [id, label, spec, can carry alpha]
const CODECS = [
  { id: "h264", name: "H.264", ext: "mp4", types: [["hq", "Haute qualité", "CRF 16 · 4:2:0 · 8 bits"], ["std", "Standard", "CRF 20 · 4:2:0 · 8 bits"], ["small", "Léger", "CRF 26 · 4:2:0 · 8 bits"]] },
  { id: "h265", name: "H.265 / HEVC", ext: "mp4", types: [["hq", "Haute qualité", "CQ 18 · encodé par la carte graphique"], ["std", "Standard", "CQ 22 · encodé par la carte graphique"], ["small", "Léger", "CQ 28 · encodé par la carte graphique"]] },
  { id: "av1", name: "AV1", ext: "mp4", types: [["hq", "Haute qualité", "CRF 24 · 4:2:0 · 10 bits"], ["std", "Standard", "CRF 32 · 4:2:0 · 10 bits"], ["small", "Léger", "CRF 40 · 4:2:0 · 10 bits"]] },
  { id: "prores", name: "Apple ProRes", ext: "mov", types: [["proxy", "Proxy", "4:2:2 · 10 bits"], ["lt", "LT", "4:2:2 · 10 bits"], ["422", "422", "4:2:2 · 10 bits"], ["hq", "422 HQ", "4:2:2 · 10 bits"], ["4444", "4444", "4:4:4 · 12 bits · alpha", true], ["xq", "4444 XQ", "4:4:4 · 12 bits · alpha", true]] },
  { id: "dnxhr", name: "Avid DNxHR", ext: "mov", types: [["lb", "LB", "4:2:2 · 8 bits"], ["sq", "SQ", "4:2:2 · 8 bits"], ["hq", "HQ", "4:2:2 · 8 bits"], ["hqx", "HQX", "4:2:2 · 10 bits"], ["444", "444", "4:4:4 · 10 bits"]] },
  { id: "qtrle", name: "QuickTime Animation", ext: "mov", types: [["rgb", "RVB", "Sans perte · alpha", true]] },
  { id: "gif", name: "GIF animé", ext: "gif", types: [["full", "Taille d'origine", "Une palette par image"], ["small", "Léger", "640 px · 15 im/s"]] },
  { id: "mp3", name: "MP3", ext: "mp3", audio: true, types: [["320", "320 kb/s", ""], ["192", "192 kb/s", ""]] },
  { id: "aac", name: "AAC", ext: "m4a", audio: true, types: [["256", "256 kb/s", ""], ["128", "128 kb/s", ""]] },
  { id: "wav", name: "WAV", ext: "wav", audio: true, types: [["16", "16 bits", "PCM"], ["24", "24 bits", "PCM"], ["32f", "32 bits float", "PCM"]] },
  { id: "flac", name: "FLAC", ext: "flac", audio: true, types: [["src", "Sans perte", "Même profondeur que la source"]] },
];
const FPS = ["23.976", "24", "25", "29.97", "30", "50", "60"];
const conv = { codec: "h264", type: { h264: "std" }, source: "file", fps: "25", seq: null, seqLabel: "" };
const segButtons = (box, items, current, pick) => box.replaceChildren(...items.map(([v, label]) => {
  const b = el("button", v === current ? "on" : "", label);
  b.onclick = () => { pick(v); renderConv(); };
  return b;
}));
function renderConv() {
  const seq = conv.source === "seq", audioOnly = !seq && stage.classList.contains("audio");
  let codec = CODECS.find(c => c.id === conv.codec);
  if ((seq || audioOnly) && (seq ? codec.audio : !codec.audio)) codec = CODECS.find(c => seq ? !c.audio : c.audio), conv.codec = codec.id;
  const list = [];
  for (const [title, audio] of [["Vidéo", false], ["Audio", true]]) {
    list.push(el("h3", "", title));
    for (const c of CODECS.filter(c => !!c.audio === audio)) {
      const b = el("button", c.id === conv.codec ? "on" : "");
      b.append(el("span", "", c.name), el("small", "", "." + c.ext));
      b.disabled = audio ? seq : audioOnly;
      b.onclick = () => { conv.codec = c.id; renderConv(); };
      b.ondblclick = () => $("convGo").click();
      list.push(b);
    }
  }
  $("convCodecs").replaceChildren(...list);
  const typeId = codec.types.some(t => t[0] === conv.type[codec.id]) ? conv.type[codec.id] : codec.types[Math.min(1, codec.types.length - 1)][0];
  conv.type[codec.id] = typeId;
  const type = codec.types.find(t => t[0] === typeId);
  segButtons($("convTypes"), codec.types, typeId, v => conv.type[codec.id] = v);
  $("convSpec").replaceChildren(...type[2].split(" · ").map(part => el("span", "", part)));
  $("convSpec").parentElement.hidden = !type[2];
  segButtons($("convFps"), FPS.map(f => [f, f]), conv.fps, v => conv.fps = v);
  $("convFpsRow").hidden = !seq;
  $("convAlphaRow").hidden = !type[3];
  $("convSrc").textContent = seq ? conv.seqLabel || "Aucune séquence choisie" : st.filename || "";
  $("convPick").hidden = !seq;
  $("convGo").disabled = seq && !conv.seq;
  const stem = seq ? (conv.seqLabel.split(/[#.]/)[0].replace(/[_\-.]+$/, "") || "sequence") : (st.filename || "").replace(/\.[^.]+$/, "");
  $("convOut").textContent = `→ ${stem}_${codec.id}_${typeId}.${codec.ext}`;
  document.querySelectorAll("#convSource button").forEach(b => b.classList.toggle("on", b.dataset.src === conv.source));
  moveTabPill($("convSource"));
}
document.querySelectorAll("#convSource button").forEach(b => b.onclick = () => { conv.source = b.dataset.src; renderConv(); });
$("convPick").onclick = () => send({ t: "pick-seq" });
vb.seqPicked = (url, label) => { conv.seq = url; conv.seqLabel = label; renderConv(); };
$("convBtn").onclick = () => {
  closeNav();
  conv.source = stage.classList.contains("idle") ? "seq" : "file";
  $("convAlpha").checked = !$("alphaBtn").hidden; // the source has transparency: keep it by default
  renderConv(); modalOpen("convBack");
};
$("convGo").onclick = () => {
  if (conv.source === "file" && stage.classList.contains("idle")) { toast("Ouvrez d'abord un fichier à convertir"); return; }
  closeModal($("convBack"));
  const codec = CODECS.find(c => c.id === conv.codec), type = codec.types.find(t => t[0] === conv.type[codec.id]);
  const seq = conv.source === "seq";
  send({ t: "convert", preset: `${codec.id}:${type[0]}`, alpha: !!type[3] && $("convAlpha").checked, seq: seq ? conv.seq : null, fps: seq ? conv.fps : null });
};

// Language: saved in settings.json by Rust (localStorage in a plain browser preview).
function setLang(code) {
  i18n.set(code);
  fitTitleBar(); // English labels have other widths
  send({ t: "lang", v: i18n.lang });
  try { localStorage.setItem("lang", i18n.lang); } catch {}
}
vb.lang = saved => {
  let stored = null;
  try { stored = localStorage.getItem("lang"); } catch {}
  setLang(saved || stored || (navigator.language.toLowerCase().startsWith("fr") ? "fr" : "en"));
};

// --- Playlist: mpv's own list (one file after another; keep-open=yes moves on, stops after the last).
let plKey = "", plDrag = null;
const plEntries = () => json("playlist") || [];
function togglePlaylist(open = !$("playlist").classList.contains("open")) {
  $("playlist").classList.toggle("open", open);
  $("listBtn").classList.toggle("on", open);
  renderPlaylist();
}
function renderPlaylist() {
  if (plDrag || !$("playlist").classList.contains("open")) return;
  const list = plEntries();
  const key = JSON.stringify(list.map(e => [e.filename, !!e.current]));
  if (key === plKey) return;
  plKey = key;
  $("plCount").textContent = list.length ? String(list.length) : "";
  $("plEmpty").hidden = list.length > 0;
  $("plClear").hidden = list.length < 1;
  $("plItems").replaceChildren(...list.map((e, i) => {
    const li = el("li", e.current ? "current" : "");
    li.dataset.i = i;
    const rm = el("button", "rm"); rm.setAttribute("aria-label", "Retirer");
    const x = el("i", "ico"); x.dataset.i = "x"; setSrc(x); rm.append(x);
    rm.onclick = ev => { ev.stopPropagation(); cmd("playlist-remove", i); };
    li.append(el("span", "n", String(i + 1)), el("span", "name", e.title || (e.filename || "").split(/[\\/]/).pop()), rm);
    li.title = e.filename || "";
    return li;
  }));
}
$("listBtn").onclick = () => togglePlaylist();
// Empties the list and stops: back to the home screen, like the logo.
$("plClear").onclick = ev => { ev.stopPropagation(); cmd("stop"); };
$("plAdd").onclick = () => send({ t: "open-files", append: true });
// Files dragged over the window (Rust gives the cursor in physical pixels). With the list open,
// a line follows the cursor between rows and the files land there (at the end outside the list);
// with it closed they replace what plays.
function plDropIndex(x, y) {
  if (!$("playlist").classList.contains("open")) return null;
  const cy = y / devicePixelRatio, cx = x / devicePixelRatio, rows = [...$("plItems").children];
  const box = $("playlist").getBoundingClientRect();
  if (cx < box.left || cx > box.right || cy < box.top || cy > box.bottom) return rows.length;
  for (const r of rows) { const b = r.getBoundingClientRect(); if (cy < b.top + b.height / 2) return +r.dataset.i; }
  return rows.length;
}
function plDropMark(at) {
  stage.classList.toggle("drop-list", at !== null);
  const rows = [...$("plItems").children];
  rows.forEach(r => r.classList.remove("drop-before", "drop-after"));
  if (at === null || !rows.length) return;
  if (at < rows.length) rows[at].classList.add("drop-before"); else rows.at(-1).classList.add("drop-after");
}
vb.dropped = (paths, x, y) => {
  const at = plDropIndex(x ?? 0, y ?? 0);
  plDropMark(null);
  send(at === null ? { t: "load-paths", paths, append: false } : { t: "load-paths", paths, at });
};
// Click a row: play it. Drag a row: move it (a line shows where it lands).
$("plItems").addEventListener("pointerdown", ev => {
  const li = ev.target.closest("li");
  if (!li || ev.button !== 0 || ev.target.closest(".rm")) return;
  plDrag = { from: +li.dataset.i, y: ev.clientY, moved: false, to: null, li };
  li.setPointerCapture(ev.pointerId);
});
$("plItems").addEventListener("pointermove", ev => {
  if (!plDrag) return;
  if (!plDrag.moved && Math.abs(ev.clientY - plDrag.y) < 5) return;
  plDrag.moved = true; plDrag.li.classList.add("lifted");
  const rows = [...$("plItems").children];
  rows.forEach(r => r.classList.remove("drop-before", "drop-after"));
  let to = rows.length;
  for (const r of rows) { const b = r.getBoundingClientRect(); if (ev.clientY < b.top + b.height / 2) { to = +r.dataset.i; break; } }
  plDrag.to = to;
  if (to < rows.length) rows[to].classList.add("drop-before"); else rows.at(-1)?.classList.add("drop-after");
});
$("plItems").addEventListener("pointerup", () => {
  const d = plDrag; plDrag = null; if (!d) return;
  [...$("plItems").children].forEach(r => r.classList.remove("drop-before", "drop-after", "lifted"));
  if (!d.moved) cmd("playlist-play-index", d.from);
  else if (d.to !== null && d.to !== d.from && d.to !== d.from + 1) cmd("playlist-move", d.from, d.to); // lands before entry `to`
  plKey = ""; renderPlaylist();
});

// The logo (Accueil): stops playback and empties the playlist, back to the mascot screen.
$("homeBtn").onclick = () => { closeNav(); cmd("stop"); };
$("infoBtn").onclick = () => {
  closeNav(); modalOpen("infoBack"); send({ t: "info" });
  requestAnimationFrame(() => moveTabPill());
  clearInterval(infoTimer); infoTimer = setInterval(() => { if (infoTab === "stats") send({ t: "info" }); }, 1000);
};

// --- Mask for projection mapping: an image laid over the picture.
// A PNG with transparency is shown as is. An opaque image (JPG, flat PNG) is read as a black and white
// mask: black hides the video, white lets it through (luminance turned into transparency).
const maskCanvas = $("mask");
let maskLoaded = false, maskShown = false, maskInverted = false, maskAlpha = null;
function layoutMask() {
  const od = json("osd-dimensions");
  if (!od || !od.w) return;
  const k = window.devicePixelRatio || 1;
  Object.assign(maskCanvas.style, {
    left: od.ml / k + "px", top: od.mt / k + "px",
    width: (od.w - od.ml - od.mr) / k + "px", height: (od.h - od.mt - od.mb) / k + "px",
  });
}
function syncMaskMenu() {
  $("maskToggle").classList.toggle("disabled", !maskLoaded);
  $("maskRemove").classList.toggle("disabled", !maskLoaded);
  $("maskInvert").classList.toggle("disabled", !maskLoaded);
  $("maskInvert").querySelector("b").textContent = maskInverted ? "Rétablir le masque" : "Inverser le masque";
  $("maskToggle").querySelector("b").textContent = maskShown ? "Cacher le masque" : "Afficher le masque";
  maskCanvas.classList.toggle("on", maskLoaded && maskShown);
}
vb.mask = (url, name) => {
  const img = new Image();
  img.onload = () => {
    maskCanvas.width = img.naturalWidth; maskCanvas.height = img.naturalHeight;
    const ctx = maskCanvas.getContext("2d", { willReadFrequently: true });
    ctx.clearRect(0, 0, img.naturalWidth, img.naturalHeight);
    ctx.drawImage(img, 0, 0);
    const data = ctx.getImageData(0, 0, img.naturalWidth, img.naturalHeight), px = data.data;
    let hasAlpha = false;
    for (let i = 3; i < px.length; i += 4) if (px[i] < 255) { hasAlpha = true; break; }
    if (!hasAlpha) {
      for (let i = 0; i < px.length; i += 4) {
        const lum = 0.2126 * px[i] + 0.7152 * px[i + 1] + 0.0722 * px[i + 2];
        px[i] = px[i + 1] = px[i + 2] = 0; px[i + 3] = 255 - lum;
      }
    }
    maskAlpha = data; maskInverted = false;
    ctx.putImageData(data, 0, 0);
    maskLoaded = maskShown = true;
    $("maskName").textContent = name;
    syncMaskMenu(); layoutMask();
    toast(hasAlpha ? "Masque importé" : "Masque noir et blanc importé : le noir cache la vidéo");
  };
  img.onerror = () => toast("Impossible de lire cette image");
  img.src = url;
};
Object.assign(actions, {
  importMask: () => send({ t: "import-mask" }),
  // Invert: every pixel's transparency flips (hidden ↔ visible); colours of a PNG are kept.
  invertMask: () => {
    if (!maskLoaded || !maskAlpha) return;
    const px = maskAlpha.data;
    for (let i = 3; i < px.length; i += 4) px[i] = 255 - px[i];
    maskCanvas.getContext("2d").putImageData(maskAlpha, 0, 0);
    maskInverted = !maskInverted; syncMaskMenu();
    toast(maskInverted ? "Masque inversé" : "Masque rétabli");
  },
  toggleMask: () => { if (!maskLoaded) return; maskShown = !maskShown; syncMaskMenu(); toast(maskShown ? "Masque affiché" : "Masque caché"); },
  removeMask: () => { maskLoaded = maskShown = maskInverted = false; maskAlpha = null; $("maskName").textContent = "Aucun masque"; syncMaskMenu(); toast("Masque retiré"); },
});
syncMaskMenu();

// --- 360° view: mpv projects the sphere (src/360.glsl); drag the picture to look around, wheel to zoom.
var v360 = false, v360Key = null, looked = false;
let view, look = null, viewRaf = 0;
const rad = d => d * Math.PI / 180;
const sendView = () => { viewRaf = 0; send({ t: "set", k: "glsl-shader-opts", v: `yaw=${rad(view.yaw)},pitch=${rad(view.pitch)},fov=${rad(view.fov)}` }); };
const queueView = () => { viewRaf ||= requestAnimationFrame(sendView); };
function set360(on, quiet) {
  v360 = on; view = { yaw: 0, pitch: 0, fov: 100 };
  if (on) sendView();
  send({ t: "view360", on });
  // Redraw at the screen's rate, not the video's: turning the view stays smooth during playback.
  send({ t: "set", k: "video-sync", v: on ? "display-resample" : "audio" });
  send({ t: "set", k: "interpolation", v: on ? "yes" : "no" });
  stage.classList.toggle("v360", on);
  $("v360Toggle").querySelector("b").textContent = on ? "Quitter la vue 360°" : "Vue 360°";
  if (!quiet) toast(on ? "Vue 360° : glissez pour regarder autour" : "Vue 360° désactivée");
}
actions.toggle360 = () => set360(!v360);
vb.spherical = (path, on) => { if (on && path === st.path && !v360) set360(true); };
stage.addEventListener("pointerdown", ev => {
  if (!v360 || ev.button !== 0 || ev.target.closest(".controls, .menu")) return;
  look = { x: ev.clientX, y: ev.clientY }; looked = false; stage.setPointerCapture(ev.pointerId);
});
stage.addEventListener("pointermove", ev => {
  if (!look) return;
  const dx = ev.clientX - look.x, dy = ev.clientY - look.y;
  if (!looked && Math.hypot(dx, dy) < 4) return;
  looked = true; look = { x: ev.clientX, y: ev.clientY };
  const k = view.fov / stage.clientWidth;
  view.yaw = (view.yaw - dx * k) % 360;
  view.pitch = Math.max(-90, Math.min(90, view.pitch + dy * k));
  queueView();
});
stage.addEventListener("pointerup", () => { look = null; });
stage.addEventListener("wheel", ev => {
  if (!v360 || ev.target.closest(".controls, .menu")) return;
  ev.preventDefault();
  view.fov = Math.max(30, Math.min(140, view.fov * (ev.deltaY > 0 ? 1.1 : 1 / 1.1)));
  queueView();
}, { passive: false });

// Fullscreen: show the title bar when the mouse touches the top edge, hide it once it moves away.
addEventListener("mousemove", ev => {
  if (!win.classList.contains("fullscreen")) return;
  const menuOpen = navItems.some(i => i.classList.contains("open"));
  if (ev.clientY <= 6) win.classList.add("peek");
  else if (ev.clientY > 70 && !menuOpen && !grabbing) win.classList.remove("peek");
});

send({ t: "ready" });

// --- Mascot gestures: now and then (every 15-35 s) the cloud turns around, hops, wobbles or shrinks and grows back.
const GESTURES = [
  [[{ transform: "rotate(0)" }, { transform: "rotate(360deg)" }], 2200, "cubic-bezier(.65,0,.35,1)"],
  [[{ transform: "none" }, { transform: "scale(1.07,.93)", offset: .25 }, { transform: "translateY(-14px) scale(.95,1.06)", offset: .55 }, { transform: "none" }], 1100, "ease-in-out"],
  [[{ transform: "rotate(0)" }, { transform: "rotate(-8deg)", offset: .25 }, { transform: "rotate(6deg)", offset: .55 }, { transform: "rotate(-2deg)", offset: .8 }, { transform: "rotate(0)" }], 1800, "ease-in-out"],
  [[{ transform: "scale(1)" }, { transform: "scale(.88)", offset: .4 }, { transform: "scale(1.05)", offset: .75 }, { transform: "scale(1)" }], 1600, "ease-in-out"],
];
// Slow life (breathing, turning colours): moves of a few px per second, so 8 updates a second look
// as fluid as 60 and the compositor draws 7x fewer frames. Nothing runs while the mascot is hidden.
const flows = [...document.querySelectorAll(".cloud .flow")];
setInterval(() => {
  const m = $("mascotWrap");
  if (!m.offsetParent || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const t = performance.now() / 1000;
  m.style.transform = `scale(${(1 + 0.015 * (1 - Math.cos(t * Math.PI * 2 / 7))).toFixed(4)})`;
  flows[0].style.transform = `rotate(${(t * 360 / 173 % 360).toFixed(2)}deg)`;
  flows[1].style.transform = `scaleX(-1) rotate(${(t * 360 / 131 % 360).toFixed(2)}deg)`;
}, 125);
(function gesture() {
  setTimeout(() => {
    const m = $("mascotWrap");
    if (m.offsetParent && !matchMedia("(prefers-reduced-motion: reduce)").matches) {
      const [frames, duration, easing] = GESTURES[Math.floor(Math.random() * GESTURES.length)];
      m.animate(frames, { duration, easing, composite: "add" }); // on top of the breathing
    }
    gesture();
  }, 15000 + Math.random() * 20000);
})();

// Controls hidden: Rust stops sending the playback clock (up to 30 page updates a second for
// nothing on screen) and sends the latest values as soon as they show again.
let uiQuiet = false;
new MutationObserver(() => {
  const quiet = stage.classList.contains("hide-ui");
  if (quiet !== uiQuiet) { uiQuiet = quiet; send({ t: "quiet", on: quiet }); }
}).observe(stage, { attributes: true, attributeFilter: ["class"] });

// Player bar too narrow for both pills: playlist, subtitles and checkerboard fold into the settings.
function fitBar() {
  const bar = document.querySelector(".bar");
  bar.classList.remove("compact", "tight");
  const [left, , right] = bar.children;
  const over = () => left.scrollWidth + right.scrollWidth + 16 > bar.clientWidth;
  if (over()) bar.classList.add("compact");
  if (over()) bar.classList.add("tight"); // still too wide: frame number, volume slider and frame steps go
}
new ResizeObserver(fitBar).observe(document.querySelector(".bar"));
new MutationObserver(fitBar).observe($("alphaBtn"), { attributes: true, attributeFilter: ["hidden"] });

// Plain browser preview (no Rust behind): pick the language right away.
if (!window.ipc) vb.lang(null);
