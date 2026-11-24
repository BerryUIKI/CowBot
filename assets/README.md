# CowBot assets

Keep shared visual assets here so documentation and future application surfaces can reuse the same artwork.

## Brand files

| File | Role |
| --- | --- |
| [`brand/cowbot-icon.svg`](brand/cowbot-icon.svg) | Original standalone icon, preserved unchanged. Hand-drawn cubic Bézier paths and geometric shapes, a transparent canvas, and a `1000 × 1000` viewBox. |
| [`brand/cowbot-icon.png`](brand/cowbot-icon.png) | Retained `512 × 512` export on a warm light background (`#f4f3ee`). The README uses the transparent wordmarks below. |
| [`brand/cowbot-wordmark-light.svg`](brand/cowbot-wordmark-light.svg) | Transparent horizontal README wordmark: monochrome cow and serif `CowBot` lettering in near-black (`#272622`). |
| [`brand/cowbot-wordmark-dark.svg`](brand/cowbot-wordmark-dark.svg) | Matching README wordmark in warm white (`#f0eee7`) for dark backgrounds. |

The rounded cow head, eye patch, and two inspiration strokes form the icon. The artwork uses near-black (`#272622`) and white. Keep its proportions and clear space; the cow's white interior is intentional, while the SVG's exterior is transparent.

The README wordmarks are separate variants with a `618 × 164` viewBox, no background tile, and no enclosing border. Their cow interiors are transparent cutouts, so the artwork uses a single ink color in either theme. An SVG luminance mask preserves the layered cow geometry without changing the original icon. The lettering is set in Georgia Bold and stored as vector outlines; no font file or installed font is required to display it. The README's `<picture>` element selects the matching wordmark using `prefers-color-scheme`, with the light variant as the fallback.

## Updating assets

1. Keep the original standalone SVG unchanged for README-only design changes. Edit the separate wordmark variants directly, keeping their geometry identical and changing only the ink color between themes. Keep meaningful element IDs and the accessible title/description. Use vector paths and shapes; do not embed raster images, fonts, scripts, or external resources.
2. If the standalone icon itself is intentionally revised in a future design change, regenerate its PNG rather than editing the export. With ImageMagick 7 and SVG support:

   ```bash
   magick -background "#f4f3ee" assets/brand/cowbot-icon.svg -resize 512x512 -depth 8 assets/brand/cowbot-icon.png
   ```

3. Check the icon at small sizes and preview the README in light and dark themes. Keep the source, derived export, and references together in the same small PR targeting `main`.

Use stable, lowercase, kebab-case filenames. Git history records revisions; add a descriptive suffix only for a genuinely different asset variant. Link assets with relative repository paths so branch and release previews work without remote hosting.

Exploratory drafts, tracing experiments, and temporary previews belong in the ignored root `output/` or `tmp/` directories. Commit only maintained sources and the exports consumed by the project. These small files use ordinary Git; no asset pipeline or Git LFS is required for the current icon set.
