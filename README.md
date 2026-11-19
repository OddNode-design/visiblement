<p align="center"><img src="assets/icon.png" width="160" alt="Visiblement"></p>

<h1 align="center">Visiblement</h1>

<p align="center">Un lecteur vidéo léger et rapide pour Windows. Il lit à peu près tout, visiblement.</p>

- **Tous les formats** que FFmpeg sait lire : mp4, mov (ProRes inclus), mkv, webm, avi, gif, webp animé, apng, images…
- **Rapide** : décodage GPU (NVDEC, D3D11VA…) avec repli automatique sur le CPU, rendu `gpu-next`, basé sur [libmpv](https://mpv.io).
- **Le dossier en playlist** : précédent / suivant passent aux autres vidéos du même dossier.
- **Frame par frame** : numéro de frame dans la barre du haut, `,` et `.` pour reculer / avancer d'une image.
- **Clic droit** : enregistrer la frame affichée (PNG, JPG, WebP) ou la copier dans le presse-papiers.
- **Mises à jour** : au lancement, une fenêtre propose la nouvelle version si elle existe.
- **Rien à installer** : un dossier à dézipper.

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
| F ou double-clic | Plein écran |
| S | Sous-titres |
| L | Lire en boucle |
| Ctrl+S | Enregistrer la frame sous… |
| Ctrl+C | Copier la frame |
| K | Tous les raccourcis |
| Q | Quitter |

## Compiler

```powershell
cargo build --release
```

À côté de `target\release\visiblement.exe`, copier :

- `libmpv-2.dll`, tirée de l'archive `mpv-dev-x86_64-*.7z` des [builds shinchiro](https://github.com/shinchiro/mpv-winbuild-cmake/releases) (chargée à l'exécution, aucune bibliothèque n'est nécessaire pour compiler) ;
- le dossier `portable_config` de ce dépôt (interface, menu, logo).

## Crédits et licences

- Code de Visiblement : MIT.
- [mpv / libmpv](https://mpv.io) : GPLv2+ (`libmpv-2.dll`, distribuée dans les releases).
- [uosc](https://github.com/tomasklaen/uosc) : LGPL-2.1, interface incluse dans `portable_config/scripts/uosc`.
