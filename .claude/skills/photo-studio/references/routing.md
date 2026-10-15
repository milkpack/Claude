# Routing: which model for which job

Route by quality, not price. No single model wins every job, and the public evidence is thin: most
"comparisons" online come from platforms selling those models. So this table has three levels of
evidence, and anything below the top level gets a two-model A/B on its first frame.

**Evidence levels**
- **Lab**: our blind bake-off (2026-10-15): 1–2 images per model, one scene per test, a harsh
  photo-editor judge who did not know the models (notes at the end). Blind and on our kind of
  work, but tiny: gaps of 2 points or less in a test are ties.
- **Docs**: the vendor's own documentation or OpenArt's model description.
- **Weak**: practitioner reports that survived a fact-check, mostly single sources.

## The A/B rule

For a new kind of shot, or any hero image: run the **first frame on both models** of its row,
1 image each at the final settings, review them (review.md; in widget hosts the user picks), then
lock the winner for the rest of the job or series. It costs one extra job and replaces guessing.
No row is exempt today; skip only when the user names a model or has already picked one for this
kind of shot.

## Routing table

| category | first choice | A/B partner | evidence | notes |
|---|---|---|---|---|
| Candid people, flash, party, family, lifestyle | `nano-banana-pro` | `gpt-image-2` | Lab (tie: NB Pro 47, GPT 2 46, NB 2.1 46) | NB Pro and NB 2.1 followed the light brief best and most consistently; GPT Image 2 made the single best frame but its second was weak. Wan last |
| Editorial portrait, fashion on location | `nano-banana-pro` | `gpt-image-2` | Weak (Lab shows the same trend on candid) | same strengths as above; GPT 2 has the higher ceiling, NB Pro the steadier floor |
| Studio portrait, headshot, beauty close-up | `gpt-image-2` | `nano-banana-pro` | Weak | watch skin: both can go waxy on young faces (lab tell #2) |
| Group (3+ people) | `nano-banana-pro` | `gpt-image-2` | Docs | stay within 5 characters on Pro; frame so faces are large |
| Same person, new scene (identity series) | `nano-banana-pro` | `byte-plus-seedream-5-pro` | Lab (tie: 61 vs 61) | both kept likeness without pasting the face. Sunburst copied the face and expression from the reference and composited it onto a cliché scene; NB 2.1 drifted; Wan lost the likeness |
| Precise edit of an existing photo (change X, keep the rest) | `ideogram-4-5` | `gpt-image-2-5-sunburst` | Lab (1 image each, GPT-made source) | judge: Ideogram 36, Sunburst 34 of 40 (a tie); global drift near-tied for all once grain is removed; difference maps cleanest on Sunburst and Ideogram, while NB 2.1/Pro and Grok redrew faces and background. NB also shifted crop and black level; Grok darkened the grade. Always A/B |
| Expression or mood change on the owner's photos (source + mood reference) | `nano-banana-2-1` | `nano-banana-pro` | Docs (references, ≤14) + the owner's workflow | keep it, but restart from the source for each round and check faces against the source; if drift shows, try `ideogram-4-5` (Lab: lowest drift) |
| Product packshot, e-commerce, luxury detail | `nano-banana-pro` | `gpt-image-2-5-sunburst` | Weak + Docs | 2K minimum, 4K for small products; composite labels/logos back if they must be exact |
| Text in the image (poster, packaging, signage) | `ideogram-4-5` | `nano-banana-pro` | Docs | text in quotes, written first; GPT Image 2.5 is the third candidate for dense text. Cyrillic and other non-Latin text: check every letter, and for long or critical text set it as a real text layer afterwards |
| Upscale a finished image | same model, image2image at 4K: "Upscale image 1 to 4K. Keep everything exactly the same; keep natural skin texture and grain." | `gpt-image-2-5-sunburst` at 4k | none (untested) | generative upscaling invents detail: compare at 100% with the original and composite back faces that changed |
| Interiors, architecture, real estate | `nano-banana-pro` | `gpt-image-2-5-sunburst` | none | no usable evidence; the only report says GPT 2.5 re-renders whole rooms on edits. A/B every time; for staging edits use the precise-edit row and composite unstaged areas back |
| Reframe / extend to another aspect ratio | `grok-imagine-image-2` | `gpt-image-2-5-sunburst` | Docs (Grok) | Grok's Smart Resize is built for it but tops out at 2K and has no 4:5 or 21:9; use Sunburst (has 21:9, 4K) for those or for a 4K master; check faces after either |
| Background removal, region edits on objects | `grok-imagine-image-2` | `ideogram-4-5` | Docs | not on skin-critical work |
| Matched set from one brief (lookbook, series) | `kling-3-omni` (`resultType: series`) | `nano-banana-pro` | Docs | lock look with a reference sheet for people |
| Stylised, anime, illustration, Asian-leaning aesthetics | `byte-plus-seedream-5-pro` | `byte-plus-seedream-4-5` | Docs | not the first pick for photoreal people from scratch |
| Precise facial anatomy (specific face shape or features from a description) | `nano-banana-pro` | `wan2-7-image` | OpenArt's description only, contradicted by Lab | Wan was last in both people tests (glossy, HDR-crunched, wrong shadows). Use only when the face description is the whole job, and review hard |

**Not on OpenArt MCP (2026-10-15): FLUX 3.** The owner rates it highly for edits, and one
practitioner drift test put it next to Ideogram 4.5 for small edits. It can't be called through
this MCP; if a FLUX provider (BFL or fal) is added to Photo Studio, it becomes the A/B partner for
the precise-edit and expression-change rows. Re-check `openart_model_list` occasionally: OpenArt
adds models without notice.

Never route photoreal people to `nano-banana-2-lite` (no character slots) or Wan by default.

## Chains

Use a chain when one model can't do everything well. Each hop re-renders, so carry the keep list
forward and compare every hop with its input.

| job | chain |
|---|---|
| Hero image that must also exist in other formats | best: generate each format directly from the same shot description; or reframe each format from the **approved master** (reframe row), never from a previous reframe. Grok tops out at 2K |
| Person in a new scene, then fixes | `nano-banana-pro` new scene → precise edits on `ideogram-4-5` or `gpt-image-2-5-sunburst` from that frame |
| Product hero with exact label | generate the scene → composite the real product/label back (outside the model) → optional light-matching edit |
| Poster from a photo | photo (people row) → typography on `ideogram-4-5` with the photo as image 1 |
| Series of one person | reference sheet → each scene from the sheet (not from the previous scene) |

## What the lab showed (2026-10-15)

- **Candid flash family portrait** (same prompt for all, 2 images each, sum of 6 scores out of 30):
  Nano Banana Pro 25 / 22, Nano Banana 2.1 24 / 22, GPT Image 2 27 / 19, GPT Image 2.5 Sunburst
  21 / 17, Wan 2.7 16 / 14. Sunburst ignored the hard flash shadow in both frames.
- **Does the brief → shot translation help?** (Nano Banana 2.1): the narrative shot description
  scored 24 / 22, the raw Russian brief 23 / 19, a JSON version 21 / 20. The raw brief produced a
  very convincing photo that ignored the wardrobe, lipstick and shadow; JSON added nothing. Write
  prose shots; keep them only as specific as the brief.
- **Precise edit** (lipstick and coat colour on a GPT Image 2 photo): Ideogram 4.5 36, Sunburst
  34, NB 2.1 32, NB Pro 31, Grok 27 (out of 40). Grain-free drift is near-tied (16–18% of pixels,
  mostly the requested coat); by eye the difference maps are cleanest on Sunburst and Ideogram. Caveat: the source was an
  OpenAI image, which may favour Sunburst.
- **Same woman, new scene:** Seedream 5 Pro 32 / 29, NB Pro 31 / 30, Sunburst 27 / 27 (face and
  expression pasted from the reference), NB 2.1 27 / 27 (weak likeness), Wan 24 / 19.
- **Most common AI tells across all 31 images:** flash shadows missing or impossible; skin
  rendered by age (porcelain young women next to over-sharpened older men); uniform veneer teeth
  and gibberish sign text.
