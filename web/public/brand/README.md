# Aidash brand assets

Provided by the user in `aidash-logo-kit.zip` on 2026-09-24.
The original Mesh + Accent geometry is preserved. On 2026-10-07 the colors were
updated for Precision Light, and raster icons were exported from the same SVG.

- `aidash-logo.svg`: ink `#18201F` symbol/wordmark and vermilion `#C74730` accent dash.
- `aidash-logo-on-dark.svg`: `#EEF0F1` symbol/wordmark and light vermilion `#F58B74` accent dash.
- `aidash-app-icon.svg`: ink mark and vermilion accent dash on a light `#FAFAFA` tile; the SVG favicon.
- `favicon.ico`: an ICO fallback exported from the app icon.

Keep public copies synchronized with `web/src/assets/brand`. Desktop PNG,
ICO and ICNS assets in `desktop/src-tauri/icons` use the same app icon source:

```sh
npx --package @tauri-apps/cli@2.12.1 tauri icon \
  web/src/assets/brand/aidash-app-icon.svg --output .ignore/brand-icons
```

Copy the platform files already present in `desktop/src-tauri/icons` from that
output, and use its `icon.ico` for the two web favicon fallbacks.

Keep the intrinsic aspect ratios. No font installation is required; the wordmarks
use outlined paths. Graph node symbols continue to use the existing Lucide set.
