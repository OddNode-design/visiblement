// Language: the page is written in French. In English, every text node and label is swapped
// through these tables as it appears (MutationObserver), so texts set later by app.js and the
// toasts sent by Rust follow without touching their call sites.
const EN = {
  // Navigation and menus
  "Accueil": "Home", "Média": "Media", "Éditer": "Edit", "Convertir": "Convert", "Infos": "Info",
  "Ouvrir un fichier": "Open a file", "Une vidéo, une image ou un son": "A video, an image or a sound",
  "Ouvrir plusieurs fichiers": "Open files", "Ils s'enchaînent dans la playlist": "They play one after another",
  "Ouvrir un dossier": "Open a folder", "Tous les médias du dossier, dans l'ordre": "Every media file in the folder, in order",
  "Ouvrir un flux réseau": "Open a network stream", "Adresse http, rtsp, srt, udp…": "http, rtsp, srt, udp address…",
  "Ouvrir un périphérique de capture": "Open a capture device", "Webcam, carte d'acquisition ou source NDI": "Webcam, capture card or NDI source",
  "Médias récents": "Recent media", "Les derniers fichiers ouverts": "The last files you opened",
  "Début de l'extrait ici": "Clip starts here", "Place le point d'entrée": "Sets the in point",
  "Fin de l'extrait ici": "Clip ends here", "Place le point de sortie": "Sets the out point",
  "Effacer la sélection": "Clear the selection", "Retire les points d'entrée et de sortie": "Removes the in and out points",
  "Enregistrer l'extrait sous…": "Save the clip as…", "Dans le format du fichier d'origine": "In the original file's format",
  "Importer un masque…": "Import a mask…", "PNG transparent ou image noir et blanc, par-dessus la vidéo": "Transparent PNG or black and white image, over the video",
  "Cacher le masque": "Hide the mask", "Afficher le masque": "Show the mask", "Aucun masque": "No mask",
  "Inverser le masque": "Invert the mask", "Rétablir le masque": "Restore the mask", "Ce qui était caché devient visible": "What was hidden shows",
  "Retirer le masque": "Remove the mask", "Revenir à la vidéo seule": "Back to the video alone",
  "Vue 360°": "360° view", "Quitter la vue 360°": "Leave the 360° view", "Glissez pour regarder autour, molette pour zoomer": "Drag to look around, scroll to zoom",
  "Enregistrer la frame sous…": "Save the frame as…", "PNG, JPG ou WebP, en pleine résolution": "PNG, JPG or WebP, at full resolution",
  "Copier la frame": "Copy the frame", "Prête à coller dans une autre app": "Ready to paste into another app",
  "Ouvrir un fichier…": "Open a file…", "Afficher dans le dossier": "Show in folder", "Lecture / Pause": "Play / Pause",
  "Espace": "Space", "Plein écran": "Fullscreen", "Quitter": "Quit",
  // Window and controls
  "Réduire": "Minimize", "Agrandir": "Maximize", "Fermer": "Close",
  "Glissez une vidéo ici ou faites un clic droit pour le menu": "Drop a video here, or right-click for the menu",
  "Rechercher une mise à jour": "Check for updates",
  "Précédente (Page préc.)": "Previous (Page Up)", "Image précédente (,)": "Previous frame (,)", "Lecture (Espace)": "Play (Space)",
  "Pause (Espace)": "Pause (Space)", "Image suivante (;)": "Next frame (.)", "Suivante (Page suiv.)": "Next (Page Down)",
  "Muet (M)": "Mute (M)", "Damier de transparence (T)": "Transparency checkerboard (T)", "Sous-titres (S)": "Subtitles (S)",
  "Paramètres": "Settings", "Plein écran (F)": "Fullscreen (F)",
  // Settings
  "Vitesse": "Speed", "Lire en boucle": "Loop", "Enchaîner le dossier": "Play the whole folder", "Piste audio": "Audio track",
  "Décodage": "Decoding", "Langue": "Language", "Désactivés": "Off", "Charger un fichier…": "Load a file…",
  // Update
  "Nouvelle version disponible": "New version available", "Ne plus demander": "Don't ask again", "Plus tard": "Later",
  "Installer": "Install", "Télécharger": "Download", "Recherche…": "Checking…",
  "Installation… Visiblement va redémarrer": "Installing… Visiblement will restart",
  "Échec de la mise à jour : télécharge-la depuis la page des versions": "Update failed: download it from the releases page",
  // Convert
  "Choisissez le format du nouveau fichier.": "Choose the format of the new file.", "Vidéo": "Video", "Audio seul": "Audio only",
  "Se lit partout": "Plays everywhere", "Même qualité, fichier plus léger": "Same quality, smaller file",
  "Pour le montage et l'étalonnage": "For editing and grading", "Qualité maximale, transparence possible": "Top quality, transparency possible",
  "Pour le web, transparence possible": "For the web, transparency possible", "Léger, idéal sur Apple": "Small, ideal on Apple",
  "Sans perte, même profondeur": "Lossless, same bit depth", "Sans perte, compressé": "Lossless, compressed",
  "Garder la transparence (alpha)": "Keep transparency (alpha)", "Annuler": "Cancel", "Convertir…": "Convert…",
  "Ouvrez d'abord un fichier à convertir": "Open a file to convert first", "Conversion…": "Converting…",
  "Échec de la conversion": "Conversion failed",
  "encodé par la carte graphique": "encoded by the graphics card", "15 im/s": "15 fps",
  "Mettre à jour": "Update",
  "Liste de lecture": "Playlist", "Liste de lecture (P)": "Playlist (P)", "Ajouter des fichiers…": "Add files…",
  "Glissez des vidéos ici pour les lire à la suite.": "Drop videos here to play them one after another.",
  "Dernière vidéo du dossier": "Last video of the folder", "Première vidéo du dossier": "First video of the folder",
  "Retirer": "Remove", "Lecture": "Playback",
  "Préparation de la lecture fluide": "Preparing smooth playback", "Lecture fluide prête": "Smooth playback ready",
  "Lecture fluide impossible": "Smooth playback failed", "Lecture fluide annulée": "Smooth playback cancelled", "Vider la liste": "Clear the list", "Rejouer (Espace)": "Replay (Space)",
  "Conversion": "Converting", "Arrêter": "Stop", "Arrêt…": "Stopping…",
  "Conversion annulée": "Conversion cancelled", "Export annulé": "Export cancelled",
  "Fichier ouvert": "Open file", "Séquence d'images": "Image sequence", "Source": "Source", "Type": "Type",
  "Cadence": "Frame rate", "Garder la transparence": "Keep transparency", "Audio": "Audio",
  "Haute qualité": "High quality", "Standard": "Standard", "Léger": "Small", "Taille d'origine": "Original size",
  "Une palette par image": "One palette per frame", "Sans perte": "Lossless", "RVB": "RGB", "GIF animé": "Animated GIF",
  "Sans perte · alpha": "Lossless · alpha", "Même profondeur que la source": "Same bit depth as the source",
  "Aucune séquence choisie": "No sequence chosen", "Choisir…": "Choose…",
  "Cette image n'est pas numérotée (ex. plan_0001.png)": "This image has no frame number (e.g. shot_0001.png)",
  // Network, capture
  "Entrez l'adresse d'un flux ou d'une vidéo en ligne.": "Enter the address of a stream or an online video.", "Ouvrir": "Open",
  "Périphérique de capture": "Capture device", "Recherche des caméras…": "Looking for cameras…", "Sources NDI": "NDI sources",
  "Recherche sur le réseau…": "Searching the network…", "Installez NDI Tools pour recevoir les sources NDI.": "Install NDI Tools to receive NDI sources.",
  "Sources trouvées sur le réseau :": "Sources found on the network:", "Aucune source NDI trouvée sur le réseau.": "No NDI source found on the network.",
  "Choisissez une caméra, une carte d'acquisition ou une caméra virtuelle.": "Choose a camera, a capture card or a virtual camera.",
  "Aucune webcam ni carte d'acquisition détectée.": "No webcam or capture card found.", "Aucun média récent": "No recent media",
  // Info panel
  "Général": "General", "Métadonnées": "Metadata", "Statistiques": "Statistics", "Aucun média": "No media",
  "Durée": "Duration", "Résolution": "Resolution", "Nombre d'images": "Frame count", "Débit moyen": "Average bitrate",
  "Créé le": "Created", "Modifié le": "Modified", "Ce fichier ne contient pas de métadonnées.": "This file has no metadata.",
  "Format de pixel": "Pixel format", "Fréquence": "Sample rate", "Échantillons": "Samples", "Aucune piste.": "No tracks.",
  "Débit vidéo": "Video bitrate", "Débit audio": "Audio bitrate", "Images/s affichées": "Displayed frames/s",
  "Fréquence de l'écran": "Display refresh rate", "Images perdues": "Dropped frames", "Écart audio / vidéo": "Audio / video offset",
  "Mémoire tampon": "Buffer", "Sous-titres": "Subtitles",
  // Toasts
  "Sélection effacée": "Selection cleared", "Boucle désactivée": "Loop off", "Lecture en boucle": "Looping",
  "Damier de transparence": "Transparency checkerboard", "Fond uni": "Plain background",
  "Masque importé": "Mask imported", "Masque noir et blanc importé : le noir cache la vidéo": "Black and white mask imported: black hides the video",
  "Impossible de lire cette image": "Can't read this image", "Masque inversé": "Mask inverted", "Masque rétabli": "Mask restored",
  "Masque affiché": "Mask shown", "Masque caché": "Mask hidden", "Masque retiré": "Mask removed",
  "Vue 360° : glissez pour regarder autour": "360° view: drag to look around", "Vue 360° désactivée": "360° view off",
  "Export en cours, patiente avant de fermer": "Export running, wait before closing", "Ce fichier n'existe plus": "This file no longer exists",
  "Impossible de vérifier les mises à jour": "Can't check for updates", "Impossible de vérifier les mises à jour (hors ligne ?)": "Can't check for updates (offline?)",
  "Aucun fichier ouvert": "No file open", "Choisissez un autre nom que le fichier d'origine": "Pick a name other than the original file's",
  "Échec de l'export": "Export failed", "Frame copiée": "Frame copied",
  "Runtime NDI introuvable : installez NDI Tools": "NDI runtime not found: install NDI Tools",
  "Impossible de se connecter à la source NDI": "Can't connect to the NDI source", "La source NDI n'envoie pas d'image": "The NDI source sends no picture",
  "Installation du module de lecture en ligne (yt-dlp)…": "Installing the online playback module (yt-dlp)…",
  "Impossible d'installer yt-dlp : vérifie la connexion": "Can't install yt-dlp: check your connection",
  "Adresse invalide. Exemples : https://…, rtsp://…, rtp://@:1234": "Invalid address. Examples: https://…, rtsp://…, rtp://@:1234",
};
const TYPES_EN = { "Vidéo": "Video", "Audio": "Audio", "Sous-titres": "Subtitles" };
const EN_RE = [
  [/^Début de l'extrait : (.+)$/, "Clip start: $1"], [/^Fin de l'extrait : (.+)$/, "Clip end: $1"],
  [/^Sous-titres : (.+)$/, "Subtitles: $1"], [/^Piste (\d+)$/, "Track $1"],
  [/^(Vidéo|Audio|Sous-titres) (\d+)( · active)?$/, (_, t, n, a) => `${TYPES_EN[t]} ${n}${a || ""}`],
  [/^(.+) à l'affichage · (.+) au décodage$/, "$1 on display · $2 in decoding"],
  [/^Mise à jour (.+) disponible$/, "Update $1 available"], [/^Mettre à jour vers la version (.+)$/, "Update to version $1"], [/^Téléchargement… (\d+) %$/, "Downloading… $1 %"],
  [/^Version (.+) · Rechercher une mise à jour$/, "Version $1 · Check for updates"],
  [/^est prête\. Tu as la version (.+)\.$/, "is ready. You have version $1."],
  [/^Damier de transparence \(T\) · pas d'alpha dans ce fichier$/, "Transparency checkerboard (T) · no alpha in this file"],
  [/^Export \((.+)\)… (\d+) %$/, (_, s, p) => `Exporting (${s === "fichier entier" ? "whole file" : s})… ${p} %`],
  [/^Conversion… (\d+) %$/, "Converting… $1 %"],
  [/^Export \((.+)\)$/, (_, s) => `Exporting (${s === "fichier entier" ? "whole file" : s})`],
  [/^(.+) · (\d+) images$/, "$1 · $2 frames"], [/^(.+) · (\d+) images · (\d+) manquantes$/, "$1 · $2 frames · $3 missing"],
  [/^CQ (\d+) · encodé par la carte graphique$/, "CQ $1 · encoded by the graphics card"], [/^Fichier converti : (.+)$/, "Converted: $1"],
  [/^Extrait enregistré : (.+)$/, "Clip saved: $1"], [/^Frame enregistrée(.*)$/, "Frame saved$1"],
  [/^Visiblement est à jour \((.+)\)$/, "Visiblement is up to date ($1)"], [/^Connexion à (.+)…$/, "Connecting to $1…"],
];

const i18n = (() => {
  let lang = "fr";
  const ATTRS = ["data-tip", "placeholder", "aria-label", "title"];
  const frText = new WeakMap(), enText = new WeakMap(), frAttr = new WeakMap();
  const toEn = s => {
    const k = s.replace(/ /g, " ").trim();
    if (!k) return null;
    let en = EN[k];
    if (en == null) for (const [re, rep] of EN_RE) if (re.test(k)) { en = k.replace(re, rep); break; }
    return en == null ? null : s.replace(s.trim(), en);
  };
  function text(n) {
    if (lang === "en") {
      if (enText.get(n) === n.data) return; // our own English, already done
      const en = toEn(n.data);
      if (en == null) return;
      frText.set(n, n.data); enText.set(n, en); n.data = en;
    } else if (frText.has(n)) {
      if (enText.get(n) === n.data) n.data = frText.get(n); // app.js may have written new French since
      frText.delete(n); enText.delete(n);
    }
  }
  function attrs(el) {
    for (const a of ATTRS) {
      if (!el.hasAttribute(a)) continue;
      const saved = frAttr.get(el) || {};
      const v = el.getAttribute(a);
      if (lang === "en") {
        if (saved[a] && saved[a].en === v) continue;
        const en = toEn(v);
        if (en == null) continue;
        saved[a] = { fr: v, en }; frAttr.set(el, saved); el.setAttribute(a, en);
      } else if (saved[a]) {
        if (saved[a].en === v) el.setAttribute(a, saved[a].fr);
        delete saved[a];
      }
    }
  }
  function walk(n) {
    if (n.nodeType === 3) return text(n);
    if (n.nodeType !== 1 || n.tagName === "SCRIPT" || n.tagName === "STYLE") return;
    attrs(n);
    n.childNodes.forEach(walk);
  }
  new MutationObserver(list => {
    if (lang !== "en") return;
    for (const m of list) {
      if (m.type === "characterData") text(m.target);
      else if (m.type === "attributes") attrs(m.target);
      else m.addedNodes.forEach(walk);
    }
  }).observe(document.documentElement, { subtree: true, childList: true, characterData: true, attributes: true, attributeFilter: ATTRS });
  return {
    get lang() { return lang; },
    set(l) { lang = l === "en" ? "en" : "fr"; document.documentElement.lang = lang; walk(document.body); },
  };
})();
