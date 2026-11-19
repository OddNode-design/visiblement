<p align="center"><img src="assets/icon.png" width="160" alt="Visiblement"></p>

<h1 align="center">Visiblement</h1>

<p align="center">Un lecteur vidéo léger et rapide pour Windows. Il lit à peu près tout, visiblement.</p>

- **Tous les formats** que FFmpeg sait lire : mp4, mov (ProRes inclus), mkv, webm, avi, gif, webp animé, apng, images…
- **Rapide** : décodage GPU (NVDEC, D3D11VA…) avec repli automatique sur le CPU, rendu `gpu-next`, basé sur [libmpv](https://mpv.io).
- **Transparence** : ProRes 4444, PNG, QuickTime Animation… Le bouton damier (touche `T`) affiche ou cache un damier derrière les zones transparentes.
- **Vertical, 4K, 8K** : la fenêtre s'adapte à l'écran ; décodage GPU jusqu'en 8K (HEVC, AV1, VP9 selon la carte graphique).
- **Marqueurs audio** : les marqueurs d'un WAV (cue points, marqueurs After Effects / Premiere / Audition) apparaissent sur la barre de lecture, avec leur nom au survol.
- **Mini éditeur** : `B` / `N` pour le début et la fin, `Ctrl+E` pour enregistrer l'extrait (même format que l'original : WAV 24 bits, ProRes 4444 avec alpha, H.264…).
- **Le dossier en playlist** : précédent / suivant passent aux autres vidéos du même dossier.
- **Frame par frame** : numéro de frame affiché, boutons ‹ › ou touches `,` et `.` pour reculer / avancer d'une image.
- **Clic droit** : enregistrer la frame affichée (PNG, JPG, WebP) ou la copier dans le presse-papiers.
- **Mises à jour** : au lancement, une fenêtre propose la nouvelle version si elle existe.
- **Rien à installer** : un dossier à dézipper (WebView2, qui affiche l'interface, est déjà présent sur Windows 10 et 11).

## Installation

1. Télécharger `Visiblement-windows-x64.zip` dans les [Releases](../../releases/latest).
2. Dézipper où vous voulez.
3. Ouvrir une vidéo avec `visiblement.exe` : glisser le fichier sur l'exe, « Ouvrir avec… », ou lancer l'exe puis glisser le fichier dans la fenêtre.

## Raccourcis

| Touche | Action |
|---|---|
| Espace | Lecture / pause |
| ← / → | -5 s / +5 s |
| , / . | Image précédente / suivante |
| Page préc. / Page suiv. | Vidéo précédente / suivante du dossier |
| ↑ / ↓ ou molette | Volume |
| M | Muet |
| Clic sur l'image | Lecture / pause |
| F ou double-clic | Plein écran (Échap pour sortir) |
| S | Sous-titres |
| L | Lire en boucle |
| T | Damier de transparence (vidéos alpha) |
| B / N | Début / fin de l'extrait |
| Maj en glissant sur la barre | Aimanter aux marqueurs |
| Ctrl+E | Enregistrer l'extrait sous… |
| Alt+X | Effacer la sélection |
| Ctrl+O | Ouvrir un fichier |
| Ctrl+S | Enregistrer la frame sous… |
| Ctrl+C | Copier la frame |
| Q | Quitter |

## Dépannage

Pour obtenir un journal détaillé de lecture :

```powershell
$env:VISIBLEMENT_LOG = "$env:TEMP\visiblement.log"; .\visiblement.exe video.mov
```

## Compiler

```powershell
cargo build --release
```

À côté de `target\release\visiblement.exe`, copier :

- `libmpv-2.dll`, tirée de l'archive `mpv-dev-x86_64-*.7z` des [builds shinchiro](https://github.com/shinchiro/mpv-winbuild-cmake/releases) (chargée à l'exécution, aucune bibliothèque n'est nécessaire pour compiler) ;
- le dossier `ui` de ce dépôt : l'interface (HTML/CSS/JS), modifiable sans recompiler.

mpv dessine la vidéo dans une fenêtre ; l'interface est une page web affichée par WebView2 dans une fenêtre transparente posée dessus, qui pilote mpv (voir `src/app.rs`).

## Crédits et licences

- Code de Visiblement : MIT.
- [mpv / libmpv](https://mpv.io) : GPLv2+ (`libmpv-2.dll`, distribuée dans les releases).
- [wry / tao](https://github.com/tauri-apps) (MIT / Apache-2.0) pour la fenêtre et la WebView.
- Icônes [Lucide](https://lucide.dev) (ISC).
