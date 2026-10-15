# Prompt dialects

Write the shot (shot-language.md) in the dialect of the model that will render it. Sources are
the vendors' current guides (checked 2026-10-15) and practitioner reports; "practitioners" marks
the latter. On OpenArt every job is a single call: there is no chat history to lean on, so each
prompt must carry its full context, including the preserve list on edits.

## Rules for every model

- **Prose first.** Every family here follows natural-language descriptions. Labelled lines are
  fine for GPT Image; JSON helps only where exact placement of elements or text matters.
- **Front-load.** Start with the deliverable and the subject; the first sentence weighs most.
- **No quality tags.** "8K, masterpiece, best quality, ultra-detailed, award-winning" are SD-era
  habits with no current support; models tune them out or drift toward a glossy stock look.
- **Negatives by family.** Gemini, Ideogram and Seedream: describe the positive state ("an empty
  street"), because "no cars" can draw cars. GPT Image handles a short "Avoid:" or "No …" line.
- **Text in the image:** exact words in double quotes, plus position and style.
- **Never promise undetectability.** Gemini images carry a SynthID watermark; others may carry C2PA.

## Google Nano Banana: Pro, 2.1, 2, 2 Lite

- Current template: "A photorealistic [type of shot] of a [subject] in a [setting]. [The light].
  Shot from a [camera angle] with a [lens type]." Extend it with expression, wardrobe, textures,
  colour and the moment, in plain sentences.
- Complex scenes: describe them in order (background, then the people, then the details).
- Text: put the words first in the prompt, in quotes.
- Edits: "Using image 1, change only [element] to [new state]. Keep everything else exactly the
  same, preserving the original style, lighting and composition."
- Composition from several images: "Take the [element] from image 2 and place it [where] in
  image 1. [Describe the final picture]."
- References: one role per image ("Image 1 is the identity reference for the woman; image 2 is
  only a mood and expression reference"). Slots: Pro takes up to 14 images, of which about 5–6
  objects with high fidelity and up to 5 people; 2.1 and 2 take up to 10 objects and 4 people;
  2 Lite has no character slots, so don't use it for identity.
- Aspect ratio: set it. Without one the model matches the input or falls back to 1:1.
- Known tendencies (practitioners): clean, evenly lit, HDR-like "set" look on candid scenes,
  so ask for motivated light, ordinary background clutter and visible texture; 2.1 can drift
  pink/magenta over repeated edits, so restart from the source instead of editing outputs; 1K
  softens fine product detail, so use 2K or 4K.

## OpenAI GPT Image: 2, 2.5 Flare, 2.5 Sunburst

- Structure: scene → subject → key details → constraints, with the intended use named.
  Labelled lines work well:
  ```
  Use case: candid editorial photograph
  Scene: …
  Subject: … (scale, framing, gaze, what the hands touch)
  Lighting: …
  Materials: real skin texture, worn fabric, …
  Constraints: natural colour balance; no glamorization, no heavy retouching
  Avoid: studio polish; staged look
  ```
- Realism words that work: "photorealistic", "a real photograph", "taken on a real camera",
  "iPhone photo"; "pores, wrinkles, fabric wear, imperfections"; "honest and unposed". Camera
  specs are cues for appearance, not physics. Film grain only when the look needs it.
- References: "Image 1: the person (identity). Image 2: the jacket. Put the person from image 1
  in the jacket from image 2; match lighting, perspective, scale and shadows."
- Edits: "Change only: [one thing]. Keep exactly: identity, expression, pose, framing, lighting
  direction, colour, every object not mentioned." Restate the keep list on every call. One change
  per call.
- Pixel-critical regions (a face that must not move, a logo, a label): composite the approved
  edit back into the original instead of trusting the prompt.
- Settings: `quality` "medium" for exploration, "high" for final portraits, identity edits and
  small text (about 5× the price; quote it). Sizes above 2560×1440 are experimental. Flare is
  faster; Sunburst is the quality and editing-precision tier.
- Known tendencies (practitioners): the strongest "real photo" feel for candid and documentary
  scenes; can look glossy on posed portraits; Sunburst re-renders more of the frame on small
  edits than other editors (it keeps meaning, not pixels); weaker at exact product geometry.

## ByteDance Seedream: 4.5, 5 Lite, 5 Pro

- Concise, coherent natural language: subject + action + environment, then style, colour, light,
  composition. Name the use case. Text in double quotes.
- References: "image 1", "image 2" with one operation each ("dress the person in image 1 in the
  outfit from image 2", "apply the colour palette of image 2 to image 1"). Say what to extract
  (appearance, style, product features) and then describe the new scene.
- Edits: add / remove / replace / modify, naming the target and "keeping [X] unchanged". No vague
  pronouns.
- Sets: "a series of N images", then list each.
- Fit: stylised, anime and illustration first (OpenArt's own description); 5 Pro is reported as
  less realistic on people than 4.5 and smooths skin. No 4:5.

## Alibaba Wan 2.7 Image

- Chinese or English, up to 5,000 characters. References numbered by array order: "image 1",
  "image 2" (up to 9).
- Image sets: "First image: … Second image: … The features must be consistent throughout."
- Strength: precise faces; describe faces anatomically (face shape, eye shape, brow, jaw, nose).
- `thinkingMode` only helps text-to-image without input images. Edits are native up to 2K at
  Alibaba, so treat OpenArt's 4K image2image as an upscale. `mode: "pro"` costs a flat 15 credits
  at any resolution.

## xAI Grok Imagine Image 2

- Plain-language edit instructions; up to 3 references on OpenArt.
- The output takes the first input's aspect ratio unless you set one (`auto` exists).
- Use for: reframing an existing image to a new aspect ratio, background removal, region edits on
  objects and backgrounds.
- Avoid for skin-critical work on people (practitioners report plastic skin).

## Ideogram 4.5

- Plain language for photographs. Order: one-line summary → subject (any text in quotes) → pose →
  secondary elements → setting → lighting → framing → finish. Early words weigh more; use
  visually grounded words ("beautiful" and "modern" steer nothing).
- Don't write "no X" (it tends to draw X); rephrase.
- Distorted faces come from people being too small in frame: frame closer.
- References: up to 5; the first is the image being reworked and the prompt becomes an edit
  instruction. Keep the instruction short and say what changes.
- Use for: posters, signage and packaging with exact lettering; small precise edits (practitioners
  measured the least drift per edit).

## Kling 3 Omni (image)

- Prompt up to 2,500 characters. `resultType: "series"` returns 2–9 coherent frames (useful for a
  matched set from one brief). Address references as "image 1", "image 2" with roles.
- Inexpensive 4K; no 4:5.
