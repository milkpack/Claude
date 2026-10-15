# Routing: which model for which job

Route by quality, not price. No single model wins every job, and the public evidence is thin: most
"comparisons" online come from platforms selling those models. So this table has three levels of
evidence, and anything below the top level gets a two-model A/B on its first frame.

**Evidence levels**
- **Lab**: our blind bake-off (2026-10-15, 2 images per model, a harsh photo-editor judge who did
  not know the models; see the notes at the end). Small n, but blind and on our kind of work.
- **Docs**: the vendor's own documentation or OpenArt's model description.
- **Weak**: practitioner reports that survived a fact-check, mostly single sources.

## The A/B rule

For a new kind of shot, or any hero image: run the **first frame on the top two models** of its
row (1–2 images each), review them (review.md), then lock the winner for the rest of the job or
series. This costs one extra job and replaces guessing. Skip it only for rows marked Lab with a
clear winner, or when the user names a model.

## Routing table

| category | first choice | A/B partner | evidence | notes |
|---|---|---|---|---|
| Candid people, flash, party, family, lifestyle | `nano-banana-pro` | `gpt-image-2` | Lab | NB Pro and NB 2.1 followed the light brief best and most consistently; GPT Image 2 made the single best frame but its second was weak. Wan last |
| Editorial portrait, fashion on location | `nano-banana-pro` | `gpt-image-2` | Lab (by analogy) + Weak | same strengths as above; GPT 2 has the higher ceiling, NB Pro the steadier floor |
| Studio portrait, headshot, beauty close-up | `gpt-image-2` | `nano-banana-pro` | Weak | watch skin: both can go waxy on young faces (lab tell #2) |
| Group (3+ people) | `nano-banana-pro` | `gpt-image-2` | Docs | stay within 5 characters on Pro; frame so faces are large |
| Same person, new scene (identity series) | `nano-banana-pro` | `byte-plus-seedream-5-pro` | Lab | both kept likeness without pasting the face. Sunburst copied the face and expression from the reference and composited it onto a cliché scene; NB 2.1 drifted; Wan lost the likeness |
| Precise edit of an existing photo (change X, keep the rest) | `ideogram-4-5` | `gpt-image-2-5-sunburst` | Lab | both made the change cleanly with the least drift. NB 2.1 and Pro regenerated the whole frame (crop and black-level drift); Grok drifted most and darkened the grade |
| Expression or mood change on the owner's photos (source + mood reference) | `nano-banana-2-1` | `nano-banana-pro` | the owner's own practice | keep it, but restart from the source for each round and check faces against the source; if drift shows, try `ideogram-4-5` (Lab: lowest drift) |
| Product packshot, e-commerce, luxury detail | `nano-banana-pro` | `gpt-image-2-5-sunburst` | Weak + Docs | 2K minimum, 4K for small products; composite labels/logos back if they must be exact |
| Text in the image (poster, packaging, signage) | `ideogram-4-5` | `nano-banana-pro` | Docs | text in quotes, written first; GPT Image 2.5 is the third candidate for dense text |
| Interiors, architecture, real estate | `gpt-image-2-5-sunburst` | `nano-banana-pro` | Weak | for staging edits use the precise-edit row; keep unstaged areas untouched (composite back) |
| Reframe / extend to another aspect ratio | `grok-imagine-image-2` | `ideogram-4-5` | Docs | Grok's Smart Resize is built for it; avoid it where faces must stay unchanged (it shifts grade and detail) |
| Background removal, region edits on objects | `grok-imagine-image-2` | `ideogram-4-5` | Docs | not on skin-critical work |
| Matched set from one brief (lookbook, series) | `kling-3-omni` (`resultType: series`) | `nano-banana-pro` | Docs | lock look with a reference sheet for people |
| Stylised, anime, illustration, Asian-leaning aesthetics | `byte-plus-seedream-5-pro` | `byte-plus-seedream-4-5` | Docs | not the first pick for photoreal people from scratch |
| Precise facial anatomy (specific face shape or features from a description) | `wan2-7-image` | `nano-banana-pro` | Docs, contradicted by Lab | Wan was last in both people tests (glossy, HDR-crunched, wrong shadows). Use only when the face description is the whole job, and review hard |

Never route photoreal people to `nano-banana-2-lite` (no character slots) or Wan by default.

## Chains

Use a chain when one model can't do everything well. Each hop re-renders, so carry the keep list
forward and compare every hop with its input.

| job | chain |
|---|---|
| Hero image that must also exist in other formats | generate (row above) → reframe each format from the **approved master** (`grok-imagine-image-2` or `ideogram-4-5`), never from a previous reframe |
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
  34, NB 2.1 32, NB Pro 31, Grok 27 (out of 40). Difference maps agree. Caveat: the source was an
  OpenAI image, which may favour Sunburst.
- **Same woman, new scene:** Seedream 5 Pro 32 / 29, NB Pro 31 / 30, Sunburst 27 / 27 (face and
  expression pasted from the reference), NB 2.1 27 / 27 (weak likeness), Wan 24 / 19.
- **Most common AI tells across all 31 images:** flash shadows missing or impossible; skin
  rendered by age (porcelain young women next to over-sharpened older men); uniform veneer teeth
  and gibberish sign text.
