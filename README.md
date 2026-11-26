<p align="center"><img src="assets/icon.png" width="160" alt="Visiblement"></p>

<h1 align="center">Visiblement</h1>

<p align="center">Lecteur vidéo pour Windows, construit sur mpv. Il lit à peu près tous les fichiers. Visiblement.</p>

<p align="center">Vous y glissez une vidéo, une image ou un son et il le lit avec le décodage de votre carte graphique.</p>

## Ce que vous pouvez ouvrir

Visiblement lit les formats que FFmpeg connaît : mp4, mov, mkv, webm, avi, gif, webp animé, apng, images fixes, wav, mp3, flac. Votre carte graphique décode le H.264, le HEVC, l'AV1 et le VP9 jusqu'en 8K (NVDEC ou D3D11VA). Le ProRes passe par le processeur, sur tous les cœurs.

À l'ouverture, la fenêtre prend les proportions de la vidéo sans dépasser 90 % de l'écran. Une vidéo verticale ou en 8K tient donc à l'écran.

## Ce que vous pouvez faire

- **Vérifier une transparence.** Sur un ProRes 4444 exporté avec sa couche alpha, un WebM VP9 alpha ou un PNG, le bouton damier (touche `T`) affiche un damier derrière les zones transparentes. Un ProRes 4444 exporté en « RVB » seul n'a pas d'alpha et le damier n'y montre rien.
- **Avancer image par image.** Les boutons ‹ › ou les touches `,` et `.` déplacent la lecture d'une image. Le numéro de frame reste affiché dans la barre.
- **Recevoir une source NDI.** Média › Ouvrir un périphérique de capture liste les webcams, les caméras virtuelles et les sources NDI du réseau (avec le runtime de NDI Tools installé). La vidéo NDI s'affiche en direct ; le son n'est pas encore pris en charge.
- **Parcourir un dossier.** Précédent et suivant passent aux autres fichiers du dossier de la vidéo ouverte.
- **Lire les marqueurs d'un WAV.** Les cue points et les marqueurs posés dans After Effects, Premiere ou Audition apparaissent sur la barre de lecture. Au survol, le nom du marqueur remplace le timecode. Maintenez Maj en faisant glisser la tête de lecture pour l'aimanter au marqueur le plus proche.
- **Couper un extrait.** `B` marque le début, `N` la fin, `Ctrl+E` enregistre l'extrait. Visiblement réencode dans le format d'origine : un WAV 24 bits reste en 24 bits, un ProRes 4444 garde son alpha.
- **Poser un masque.** Éditer › Importer un masque superpose une image à la vidéo, calée sur l'image. Un PNG transparent s'affiche tel quel ; un masque noir et blanc cache la vidéo là où il est noir. `Ctrl+I` inverse le masque.
- **Sauver une image.** Le clic droit enregistre la frame affichée en PNG, JPG ou WebP, ou la copie dans le presse-papiers pour la coller dans Photoshop.

Pendant la lecture d'un fichier audio, la mascotte remplace ses yeux par une forme d'onde et une sphère blanche ondule derrière elle.

## Installation

Dans les [Releases](../../releases/latest), prenez l'un des deux fichiers :

- `Visiblement-x.y.z-x64.msi` installe l'app pour votre compte, sans droits administrateur. Il ajoute un raccourci au menu Démarrer et « Ouvrir avec Visiblement » sur les vidéos, les images et l'audio. Une nouvelle version s'installe par-dessus l'ancienne.
- `Visiblement-windows-x64.zip` contient la version portable. Dézippez le dossier où vous voulez et lancez `visiblement.exe`.

Windows 10 et 11 embarquent WebView2, qui affiche l'interface. Vous n'avez rien d'autre à installer.

Au lancement, Visiblement interroge GitHub et vous propose la nouvelle version quand elle existe. Si vous cochez « Ne plus demander », le petit bouton en bas à droite de l'écran d'accueil vous permet encore de vérifier.

## Raccourcis

| Touche | Action |
|---|---|
| Espace ou clic sur l'image | Lecture / pause |
| ← / → | Reculer / avancer de 5 s |
| , / . | Image précédente / suivante |
| Page préc. / Page suiv. | Fichier précédent / suivant du dossier |
| ↑ / ↓ ou molette | Volume |
| M | Muet |
| F ou double-clic | Plein écran (Échap pour sortir) |
| S | Sous-titres |
| L | Lecture en boucle |
| T | Damier de transparence |
| B / N | Début / fin de l'extrait |
| Maj en glissant sur la barre | Aimanter aux marqueurs |
| Ctrl+E | Enregistrer l'extrait sous… |
| Alt+X | Effacer la sélection |
| Ctrl+M | Afficher / cacher le masque |
| Ctrl+I | Inverser le masque |
| Ctrl+O | Ouvrir un fichier |
| Ctrl+S | Enregistrer la frame sous… |
| Ctrl+C | Copier la frame |
| Ctrl+Q | Quitter |

## Dépannage

Si un fichier ne se lit pas comme prévu, lancez Visiblement avec un journal de lecture et joignez-le à votre signalement :

```powershell
$env:VISIBLEMENT_LOG = "$env:TEMP\visiblement.log"; .\visiblement.exe video.mov
```

## Compiler

```powershell
cargo build --release
```

Copiez ensuite deux éléments à côté de `target\release\visiblement.exe` :

- `libmpv-2.dll`, tirée de l'archive `mpv-dev-x86_64-*.7z` des [builds shinchiro](https://github.com/shinchiro/mpv-winbuild-cmake/releases). Visiblement charge cette DLL au lancement, la compilation n'en a pas besoin.
- le dossier `ui` du dépôt. Il contient l'interface en HTML, CSS et JavaScript, que vous pouvez modifier sans recompiler.

mpv dessine l'image dans une première fenêtre. Une seconde fenêtre, transparente, se pose dessus et affiche l'interface avec WebView2. `src/app.rs` relie les deux : il transmet l'état de mpv à la page et les commandes de la page à mpv.

## Fabriquer l'installeur

Installez [WiX Toolset 5](https://wixtoolset.org) avec `dotnet tool install --global wix --version 5.0.2`, puis :

```powershell
wix build installer\visiblement.wxs -arch x64 -d Version=1.0.10 -d BinDir=target\release -d "UiDir=$PWD\ui" -o Visiblement-1.0.10-x64.msi
```

Remplacez `1.0.10` par la version inscrite dans `Cargo.toml`.

## Crédits et licences

- Code de Visiblement : MIT.
- [mpv / libmpv](https://mpv.io) : GPLv2+. Les releases distribuent `libmpv-2.dll`.
- [wry et tao](https://github.com/tauri-apps) (MIT / Apache-2.0) gèrent la fenêtre et la WebView.
- Icônes [Lucide](https://lucide.dev) (ISC).
