# Icons

Generate before the first `tauri build` (required by the bundler). From the repo root:

```powershell
npm run tauri -- icon ./assets/icon.png
```

Provide a square PNG/SVG (≥ 512×512). The command writes `icon.ico`, `icon.png`, and platform variants into this directory. Until then, `tauri dev` works without icons; `tauri build` does not.
