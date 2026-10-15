# OpenArt image models: parameters and prices

Snapshot from `openart_model_list`, `openart_model_form_get` and `openart_model_cost` on 2026-10-14.
The aggregator changes models and prices without notice, so **call `openart_model_form_get` before
the first generation with a model in a session**. If a schema disagrees with this table, the schema
wins. Prices are credits per image at the given settings. `imageCount` (1–8) multiplies the price.

| id | OpenArt's description (routing.md decides) | t2i / i2i default | expensive settings | refs (i2i) | parameter fields |
|---|---|---|---|---|---|
| `nano-banana-2-1` | people, editorial, multi-subject, posters | 30 / 32 (1K) | 4K = 80 | ≤14 | `aspectRatio` (21:9…9:16, no 9:21), `resolution` 1K/2K/4K, `autoEnhancePrompt` |
| `nano-banana-2` | same family, cheaper | 20 / 20 | 4K costs more | ≤14 | same as 2.1 |
| `nano-banana-pro` | long in-image text, multi-turn edits, ≤5 consistent people | 40 / 40 | 4K = 80 | ≤14 | same as 2.1 |
| `nano-banana-2-lite` | fast 1K only; no character slots (not for identity) | 15 / 15 | — | ≤10 | `aspectRatio` only |
| `gpt-image-2` | hyperreal people and products, luxury, tiny text | 40 / 42 (2k, medium) | `quality: high` + 4k is expensive | ≤16 | `aspectRatio` (incl. 9:21), `resolutionTier` 1k/2k/4k, `quality` low/medium/high/auto, `outputFormat`, `customWidth/Height` |
| `gpt-image-2-5-flare` | newest GPT Image, faster | 40 / 42 | as above | ≤16 | as gpt-image-2 |
| `gpt-image-2-5-sunburst` | **highest quality and most precise editing** | 40 / 42 | high + 4k = 210 (1:1), 250 (4:3), 315 (16:9) | ≤16 | as gpt-image-2 (`quality` without `auto`) |
| `wan2-7-image` | precise facial control, in-image text, flat price | 15 / 15 (pro, 4K) | — (`mode: standard` = 6, 2K max) | ≤9 | `resolution` (default 4K), `mode` pro/standard, `thinkingMode`, prompt ≤5000 chars |
| `kling-3-omni` | cheap 4K; batch production; `resultType: series` (2–9 coherent frames) | 10 / 10 (1k) | 4k = 20 | ≤10 | `resolution` 1k/2k/4k, `resultType` single/series, `seriesAmount`, prompt ≤2500 chars |
| `grok-imagine-image-2` | **editing an existing photo**: region edits, background removal, re-framing (Smart Resize) | 26 / 33 | `quality: medium`, 2K | ≤3 | `aspectRatio` incl. `auto`, 2:1, 19.5:9; `resolution` 1K/2K; `quality` low/medium |
| `byte-plus-seedream-4-5` | stylized / anime / Asian-leaning aesthetics; not first choice for realism | 15 / 15 | 4K | ≤14 | `resolution` 2K/4K |
| `byte-plus-seedream-5-pro` | premium stylized | 30 / 30 (1K) | 2K = ×2 | ≤10 | `resolution` 1K/2K |
| `byte-plus-seedream-5-lite` | stylized on a budget | 15 / 15 | — | ≤14 | — |
| `ideogram-4-5` | typography, posters, signage, logos | 44 (medium) | high = 74 (160 with refs) | ≤5 (first is the one being reworked) | `quality` very_low/low/medium/high, `resolution` 1k/2k, `aspectRatio` incl. `auto` |

## Defaults that bite

- Default resolution is **1K** on Nano Banana Pro / 2 / 2.1, Seedream 5 Pro, Kling and Grok, and
  most forms default to 1:1 or 4:3. Always send `aspectRatio` and the resolution field.
- Aspect ratios missing: **4:5** on every Seedream, Kling and Grok form; **21:9** on Grok and
  Ideogram; **9:21** only on GPT Image, Wan and Grok-style lists. Check the enum.
- Price depends on every setting, aspect ratio and `imageCount` included. Quote the exact params
  with `openart_model_cost` before any job over ~100 credits and tell the user the config.
- No form has a negative-prompt, seed, mask or transparent-background field.

## Reference images (`image2image`)

`visualReferences` is an array of objects `{type: "image", id, url, label}`. Get them from
`openart_upload_pick` (widget hosts), `openart_upload_sign` + PUT + `openart_upload_metadata_get`
(CLI or headless), `openart_upload_list` (saved uploads), or from a past generation in
`openart_creation_list` (each item has `id`, `url`). Label them `image1`, `image2`, … in the order
the prompt refers to them ("image 1", "image 2").

## Jobs

`openart_generate_image` returns a `historyId` with status `PENDING`. On a text-only host, wait with
`openart_creation_wait(historyId)`, or poll `openart_creation_get` and respect `pollAfterSeconds`.
`openart_creation_list` items carry `url` (full PNG), `thumbnailUrl` (webp), `prompt`, `input` (the
exact params), `model`, `metadata.width/height`. Successful generations land in the default project
unless you pass `projectId`.

## Leave this off

Keep `autoEnhancePrompt: false`. The skill writes the prompt itself, and the aggregator's
enhancer rewrites it into generic "AI look" phrasing.
