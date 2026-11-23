# Visiblement — reprise de session

Lecteur vidéo Windows en Rust : libmpv pour décoder et afficher, interface WebView2 (HTML/CSS/JS).
Repo public : https://github.com/OddNode-design/visiblement (branche `main`). Dernière version publiée : **0.1.16**.

## Préférences de l'utilisateur
- Répondre en français, style concis. Ne jamais ajouter Claude en co-auteur des commits.
- Montrer les changements d'interface sur le localhost **avant** de commiter.
- Publier une version seulement quand il le demande (« Publier la 0.1.x »).
- Police Roboto partout, fond des menus `#212024`, pas de contour blanc sur les panneaux flottants.

## Structure (`rplay/`)
- `src/main.rs` : chargement libmpv, réglages (`%APPDATA%\Visiblement\settings.json` : lang, volume, récents…), mises à jour, `tr()` FR/EN des dialogues Windows, détection 360°, séquences d'images.
- `src/app.rs` : fenêtres (tao + wry), messages IPC de la page (`"convert"`, `"pick-seq"`, `"lang"`, `"view360"`…).
- `src/editor.rs` : export d'extrait et **Convertir** (`preset("codec:type")`, `encode_any`, liste `PRESETS`).
- `src/360.glsl` : projection 360° (shader mpv).
- `thumbs/` : DLL COM d'aperçus pour l'Explorateur (`visiblement_thumbs.dll`, workspace Cargo).
- `ui/` : `index.html`, `styles.css`, `app.js`, `i18n.js` (table FR→EN), `fonts/Roboto.ttf`.
- `installer/visiblement.wxs` : MSI WiX 5 par utilisateur (HKCU), enregistre aussi les aperçus.

## Aperçu local de l'interface
`.claude/launch.json` (dossier parent) : serveur `visiblement-ui` sur http://localhost:8138 (sert `rplay/ui`). Sans Rust derrière, `send()` ne fait rien ; simuler l'état en JS (ex. `vb.updateAvailable({...})`, retirer la classe `idle` de `#stage`).

## Publier une version (depuis `rplay/`)
1. Version : `Cargo.toml` ligne 3 et la commande WiX du `README.md`.
2. Fermer seulement le build de dev (PowerShell : `Get-Process visiblement | Where-Object Path -like '*\target\release\*' | Stop-Process`), jamais `taskkill /IM visiblement.exe` qui ferme aussi l'app installée de l'utilisateur ; puis `cargo test --release --workspace` et `cargo build --release --workspace`.
3. `cp -r ui target/release/ui` ; vérifier la FileVersion de `target/release/visiblement.exe`.
4. MSI : `%USERPROFILE%\.dotnet\tools\wix.exe build installer\visiblement.wxs -arch x64 -d Version=X -d BinDir=target\release -d UiDir=<chemin absolu>\ui -o Visiblement-X-x64.msi`.
5. Zip portable `Visiblement-windows-x64.zip` : exe, `libmpv-2.dll`, `visiblement_thumbs.dll`, README, LICENSE, `ui/` dans un dossier `Visiblement/`.
6. `git commit`, `git tag -a vX`, `git push origin main` et le tag, `gh release create vX <msi> <zip> --notes-file …` (notes en français).

## Tests utiles
- `cargo test --release` (unitaires).
- Encodage réel de tous les formats : `cargo test --release converts_ -- --ignored` (ffmpeg/ffprobe dans le PATH).
- Aperçus : `VISIBLEMENT_MPV_DIR=<rplay>\target\release cargo test --release -p visiblement-thumbs -- --include-ignored`.
- Log mpv : variable `VISIBLEMENT_LOG=<chemin>`.

## Pièges connus
- Les éditions par heredoc Bash cassent les `\v`, `\n`… : écrire les scripts Python avec l'outil Write.
- Le libmpv livré n'a pas libx265, libvpx ni HAP ; H.265 passe par l'encodeur de la carte graphique (nvenc/amf/qsv). NotchLC : pas d'encodeur libre.
- L'encodage mpv perd la dernière image (24/25) ; une tentative de correction (keep-open) bloquait mpv.
- Aperçus Explorateur : COM par utilisateur ⇒ `DisableProcessIsolation=1` (chargé dans explorer.exe). Windows ajoute lui-même l'icône de l'app dans le coin.
- L'API GitHub sans compte est limitée à 60 requêtes/h par IP ; l'app se rabat sur la redirection `releases/latest`.

## Idées en attente
- Son des sources NDI ; HAP via un FFmpeg installé ; corriger la dernière image perdue à l'encodage.
