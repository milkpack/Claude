# Reviewing results

Look before you show. A photo editor rejects most frames; so should you.

## Getting the images in front of you

- Terminal hosts (Claude Code): download with curl from the CDN URL (`openart_creation_list`
  items carry `url` and `thumbnailUrl`), then make a JPEG with the long edge at most 1568 px for
  the full frame. Cut crops at full resolution for every face, the hands, any text and the area
  that was edited, each resized to at most 1024 px. Read the files.
- Widget hosts (Claude Desktop, claude.ai) show the result card to the user, but you can't see
  it. Ask the user which frames they prefer, or describe what to check, instead of claiming you
  reviewed it.
- Claude's vision counts approximately and can't tell AI from real by "feel". Check concrete
  things, and crop instead of trusting a downscaled full frame.

## Rubric (1–5 each)

| score | question |
|---|---|
| realism | would a photo editor accept it as a real photo at full size? |
| skin | real texture (pores, fine lines, natural shine) vs plastic, waxy, airbrushed |
| light | one physically coherent light story: direction, hardness, shadows and colour agree everywhere |
| anatomy | eyes, teeth, hands, ears, hairlines; no merged or duplicated people |
| brief | does it show what was asked (people, ages, expressions, wardrobe, setting, format)? |
| editorial | a real moment, composition, taste: would it run? |
| edit only: done | the requested change is there, fully |
| edit only: preserve | everything on the keep list unchanged (compare with the source side by side) |
| series only: identity | the same person as the canonical reference (face crop to face crop) |

Verdict: **accept** when every score is 4+ and nothing major; **retry** when a defect has a known
fix; **rethink** when the model or the approach is wrong for this shot (switch per routing.md).

## Defect → fix

| defect | fix |
|---|---|
| plastic or airbrushed skin | name the texture ("visible pores, fine lines, natural shine"), drop beauty words, step down the retouch ladder; if it persists, switch model |
| everything evenly lit, "set" look | name one source and where it sits, ask for shadow depth, add ordinary background detail |
| light incoherent (shadow direction, flash with no shadow) | describe the source position and where its shadow falls ("flash just above the lens; hard shadow on the wall behind, slightly below their heads") |
| extra, merged or duplicated people | fewer people per frame, frame closer, stay within the model's character slots |
| distorted small faces | frame closer; distant faces are where models fail |
| teeth or hands broken | frame or pose so they read clearly, or regenerate; a local edit from the source if the rest is right |
| wrong text | words first and in quotes; move the text job to the text-strong model (routing.md) |
| edit drifted outside the change | restart from the source with a fuller keep list; composite the edited region back for pixel-critical work |
| identity drift across a series | back to the canonical reference, one reference, regenerate the frame; don't edit toward the face |
| colour cast creeping over edits | restart from the source; never chain edits on the previous output |
| wrong aspect or soft detail | set `aspectRatio` and the resolution field explicitly (OpenArt defaults are 1K and 1:1 or 4:3) |

At most two fix rounds per frame. Then tell the user what isn't working and propose the
alternative (another model, a composite, a simpler shot).

## Picking from a batch

Rank the variants, show the best 1–3 with one line each on why, and say what the others got
wrong in a phrase. Keep every URL so the user can overrule you.
