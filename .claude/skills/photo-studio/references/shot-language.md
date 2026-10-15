# Shot language: from a plain brief to a shot

The client speaks in feelings ("живо", "дорого", "как в журнале"). Models respond to visible facts:
what is in the frame, where the light comes from, how big things are, what the surfaces look like.
Your job is the photographer's: turn feelings into decisions, then write the decisions as a
description of the finished photograph.

Two rules frame everything below.

1. **Describe the visible result first; camera settings are only cues.** Models do not simulate
   optics. "85mm, f/1.8" works because it shorthands a look (tight framing, soft background), and
   the look is what to describe. Say "background melts into soft blur, subject sharp" and add the
   lens as a hint, not instead of it.
2. **Add only what the brief implies.** Settle what is ambiguous and matters (light, framing, the
   moment, retouch level), and keep the client's specifics verbatim. Do not invent props, extra
   people, palettes, slogans or story the brief doesn't imply. A detail-stuffed prompt reads as
   AI. When the brief is already specific, normalise it and stop.

## 1. The shot sheet

Decide these before writing. Most briefs leave half of them open; fill the ones the model would
otherwise get wrong, and leave the rest to the model.

| slot | decide | notes |
|---|---|---|
| Use | campaign, e-commerce, editorial, social feed or story, website hero, print | drives format, retouch and how literal to be |
| Format | aspect ratio, resolution, space for copy (which side) | check the model's enum: no 4:5 on Seedream, Kling or Grok; no 21:9 on Grok or Ideogram. A4 is about 1.41:1, so use 3:2 or 4:3 and crop |
| Subject | who or what, how many, life stage, look, wardrobe, hair and make-up, expression, pose, action | life-stage words ("in her early thirties") beat bare numbers. Describe skin tone directly (depth + undertone + finish), not by ethnicity |
| Setting | location, time, season, weather, background | |
| Light | the source, its direction, hard or soft, how deep the shadows are, colour of light | describe shadow depth in words ("deep shadows on the far cheek"); models don't read ratios like 4:1 |
| Framing → lens | framing size first, then the lens that gives it | headshot 85mm, half-body 50mm, environmental 35mm, full length with soft background 135mm+ |
| Moment | posed or candid, what just happened, where eyes go | candid needs a cause ("laughing at something off-camera") |
| Medium | clean digital, phone, film look, grain, white balance | one film stock at most; it is a style hint |
| Retouch | unretouched / natural / editorial / commercial / high-end beauty | see §4 |
| Text in image | exact words in quotes, placement, style | write text first in the prompt for Gemini models |
| References | role of each image, what to take from it, what to ignore | see references.md |
| Preserve | for edits: everything that must not change | see references.md |
| Avoid | the 1–3 failure modes this shot is prone to | phrase per dialect (dialects.md) |
| Variants | how many, and what should vary between them | exploration 3–4, finals as needed (the owner runs 8 at 4K for edit rounds) |

## 2. What the client says → what to describe

| client says | describe |
|---|---|
| живо, естественно, "not AI", candid | available or motivated light, a real moment with a cause, eye-level, slightly imperfect framing, visible skin texture, ordinary background details |
| драматично, dramatic | one hard light from the side, deep shadows on the far side of the face, dark background |
| воздушно, светло, soft, airy | large soft window light or overcast daylight, open shadows, pale background, low contrast |
| кинематографично, cinematic | name the light and the atmosphere instead: practical lamps in frame, haze, a deliberate colour (e.g. sodium-orange street light against blue dusk), shallow focus; keep the chosen aspect ratio |
| дорого, luxury | controlled reflections, clean gradient or deep black background, light that draws crisp edges on the product, few props, everything in focus that matters |
| как в журнале, editorial | one strong lighting idea, styled wardrobe, a composed frame, natural retouch with texture kept |
| как с телефона, phone photo | wide phone perspective, everything in focus, bright HDR-flattened tones, heavy noise reduction (smooth, slightly smeary detail), vertical frame |
| селфи, selfie | arm's-length phone perspective, slight wide-angle distortion, face in the upper third, front-camera softness |
| вечеринка, вспышка (party flash) | on-camera flash, subject bright against a near wall with a hard shadow edge behind; warm lamps in the room glow orange (household bulbs ~2700K, candles ~1900K) |
| папарацци, red carpet, night flash | direct flash, subject bright, background falls off to near black (no wall close behind), slight motion blur in the dark areas |
| тепло, уютно, cosy | warm household lamps or low golden sun, warm white balance |
| мрачно, moody | low-key, one source, desaturated colour, cool shadows |
| чистый предметный снимок, packshot | seamless sweep (pure white for marketplaces), soft top light plus side lights drawing the edges, a soft contact shadow, front-to-back sharp |
| размытый фон, background blur | subject sharp, background a soft blur; the framing decides the lens (85mm for head and shoulders; 135mm+ or much more distance for full length) |
| групповое фото, group | everyone's face sharp and well lit, even frontal light, 50mm (35mm stretches faces at the edges); several rows → deep focus |

## 3. Vocabulary that steers the look

- **Lighting setups:** Rembrandt, loop, butterfly, split, broad vs short, clamshell (beauty), rim or hair light, contre-jour, window light, open shade, golden hour, blue hour, overcast, hard midday sun, on-camera flash, bounced flash, ring light (ring catchlights), beauty dish, softbox, strip light, coloured gels, practicals.
- **Framing and angle:** extreme close-up, close-up, medium close-up, medium (waist up), cowboy, full length, wide; eye level, low angle, high angle, top-down (flat lay), over-the-shoulder, three-quarter view, profile.
- **Lens as a look:** 24mm (space, distortion near the edges), 35mm (documentary, environment), 50mm (natural), 85mm (portrait, soft background), 100mm macro (small products, beauty detail), 135–200mm (compressed background, fashion outdoors), tilt-shift (straight verticals for architecture).
- **Depth of field in words:** "only the eyes sharp", "subject sharp, background soft", "everything sharp from front to back".
- **Motion:** "frozen mid-jump", "slight motion blur on the hands", "light trails".
- **White balance:** "neutral daylight", "warm tungsten cast", "cool blue dusk", "mixed warm and cool light".
- **Film looks (one at most):** Portra-like (warm skin, soft contrast), Ektar-like (saturated landscape colour), Kodak Gold-like (warm, nostalgic), CineStill 800T-like (night, red halation around lights), black-and-white Tri-X-like grain, medium-format look (smooth tonal roll-off).

## 4. Retouch ladder

| level | describe | use |
|---|---|---|
| unretouched | pores, fine lines, blemishes, under-eye texture, flyaway hair, natural shine | documentary, candid, the "not AI" look |
| natural | even tone, texture fully kept, stray hairs kept | editorial, family, lifestyle |
| editorial | tidy skin with pores visible, controlled shine | fashion editorial, portraits |
| commercial | clean even skin, texture still visible at 100% | advertising, e-commerce on model |
| high-end beauty | flawless make-up, sculpted light, but real skin texture at 100% | beauty campaigns only |

Words that push skin to plastic: "flawless", "perfect skin", "airbrushed", "porcelain", "smooth
skin", "8K masterpiece". Say what is there instead ("visible pores and fine lines").

## 5. Format by use

| use | aspect | resolution | notes |
|---|---|---|---|
| Instagram feed | 4:5 (3:4 where 4:5 is missing) | 2K | |
| Story, Reels cover | 9:16 | 2K | keep faces out of the top and bottom 15% |
| Website hero | 16:9 or 21:9 | 2K–4K | ask which side needs copy space |
| Editorial spread | 3:2 | 4K | |
| Print A-series | 4:3 or 3:2, crop to 1.41:1 | 4K | |
| Marketplace packshot | 1:1 or 3:4 | 2K–4K | pure white background |

OpenArt defaults are a trap: Nano Banana, Seedream 5 Pro, Kling and Grok default to 1K, and most
forms default to 1:1 or 4:3. Always set `aspectRatio` and the resolution explicitly.
