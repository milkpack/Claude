# References and edits

## Getting images in

| situation | how |
|---|---|
| a past OpenArt generation | pass it directly: `{type: "image", id: <creation item id>, url: <creation item url>, label: "image1"}` from `openart_creation_list` (verified on 10 image2image jobs) |
| a file the user has, widget host (Claude Desktop, claude.ai) | `openart_upload_pick`, wait for the upload-complete message, use the references it returns |
| a file on disk, CLI host (Claude Code) | `openart_upload_sign` → HTTP PUT the bytes to `signURL` → `openart_upload_metadata_get(mediaUrl=accessURL, mediaType="image", uploadId)` |
| something uploaded before | `openart_upload_list` |

## Labelling

- Send references in the order the prompt names them and label them `image1`, `image2`, … The
  label is free text on OpenArt, so the prompt does the real work: name each image by number and
  give it one role.
- **The image being edited goes first.** Ideogram requires it (its first reference is the one
  reworked) and it reads clearly everywhere.
- Say what to take from each image and what to ignore:
  - "image 2 is only an emotion and mood reference: do not copy its people, faces, clothes or
    background";
  - "image 3: copy only the jacket (cut, colour, fabric), not the person wearing it".
- Roles: edit source, identity, outfit, product, pose, lighting, colour/grade, style, background,
  composition sketch.

## How many

- Stay within the model's slots: Nano Banana Pro up to 5 people and about 5–6 objects at high
  fidelity; Nano Banana 2 and 2.1 up to 4 people and 10 objects; 2 Lite has no character slots;
  Grok 3 images; Ideogram 5; Wan 9; GPT Image 16.
- Identity: start from **one** canonical reference: sharp, evenly lit, frontal or three-quarter,
  face large in frame, no sunglasses or hard shadows. Add more angles only if a test shows the one
  reference isn't enough (Ideogram's docs ask for 1–5 face shots).
- For a series, build a reference sheet first (big face, neutral grey or white background, front
  and three-quarter, full length) and reuse it unchanged. A styled sheet leaks its lighting into
  every scene.

## Edits

Every edit prompt carries three parts, every time (OpenArt calls are single-turn, nothing carries over):

```
Edit image 1. Change only: <the change, concrete and visual>.
Keep exactly: <identity and expression of each person>, <pose>, <framing and crop>,
<lighting direction and shadows>, <colour grade>, <grain>, <background>, <every object not mentioned>.
Do not retouch or smooth the skin.
```

- One change per call when fidelity matters; batch changes only when they are small and far apart.
- **Restart from the source, don't chain.** Identity and colour drift accumulate with the number of
  chained edits more than with their size. Apply each new change to the original (or the last
  approved master), not to the previous output.
- **Composite pixel-critical regions back.** If a face, logo or label must stay exactly as it was,
  take the edited area from the new image and keep the rest from the original (outside OpenArt, or
  in Photo Studio) instead of asking the model to be perfect.
- Background swaps relight the face. Decide which you want and say it: "relight her to match the
  new street light" or "keep the light on her face exactly as in image 1".
- Changing the aspect ratio of an edit: Grok's output follows the first input unless `aspectRatio`
  is set; Gemini follows the input or falls back to 1:1. Set it.

## The owner's editorial pattern

A source photo plus a mood reference, many variations at 4K, then a pick. Template:

```
Edit image 1, using image 2 only as an emotion and mood reference. Keep the lighting exactly the
same as in image 1: same flash, same shadows, same colour correction, same photographic style.
Preserve the people from image 1 and keep the composition close, but change their expressions so
the image feels more alive and varied, inspired by image 2: give each person a different, natural
emotion. <Specific changes, e.g. remove the earrings, deep burgundy lips>. Keep realistic skin
texture and natural facial details. Do not over-beautify, do not make them glamorous or retouched.
```

Run 4–8 variations, review them (review.md), and edit the winner further from the original where
possible.

## Identity across a series

1. Make or pick the canonical reference (above).
2. Every scene: canonical reference as image 1 + "same person: same face shape, eyes, brows, nose,
   lips, hairline and skin tone"; describe the new scene fully; say "do not copy the background,
   lighting or other people from image 1".
3. Check every frame against the canonical reference at face crop size. Regenerate frames that
   drift rather than editing them toward the face.
4. Real people: only with their consent. Never minors in sensitive contexts, never public figures
   in invented situations.
