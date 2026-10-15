// Stand-ins for the server's Claude-backed director and review, good enough to exercise the UI.
import type { DirectorRequest, DirectorResponse, ReviewRequest, ReviewResponse } from '../api/types';
import { rng, seedOf } from './paint';

const has = (t: string, re: RegExp) => re.test(t.toLowerCase());

export function direct(req: DirectorRequest): DirectorResponse {
  const brief = req.brief.trim();
  const refs = req.references ?? [];
  const source = refs.find((r) => r.role === 'edit-source');
  const task = req.task ?? (source ? 'edit' : 'generate');
  let category = 'editorial';
  if (task === 'edit') category = 'editorial-edit';
  else if (has(brief, /poster|headline|exact text|typograph|signage/)) category = 'poster';
  else if (has(brief, /bottle|product|packshot|campaign/)) category = 'product';
  else if (has(brief, /headshot|portrait|linkedin/)) category = 'portrait';
  else if (has(brief, /series|same (man|woman|person)|four scenes/)) category = 'series';

  const plans: Record<string, DirectorResponse['plan']> = {
    'editorial-edit': [
      { model: 'nano-banana-2-1', mode: 'image2image', why: 'Best at keeping a real face while relighting and regrading from a mood reference.' },
      { model: 'gpt-image-2-5-sunburst', mode: 'image2image', why: 'Second pass for precise local fixes if identity drifts.' },
    ],
    poster: [
      { model: 'ideogram-4-5', mode: 'text2image', why: 'Sets the exact headline and small print reliably.' },
      { model: 'nano-banana-pro', mode: 'text2image', why: 'Photographic fallback with long in-image text.' },
    ],
    product: [
      { model: 'gpt-image-2-5-sunburst', mode: refs.length ? 'image2image' : 'text2image', why: 'Keeps the label and glass edges crisp at 4K.' },
      { model: 'gpt-image-2', mode: refs.length ? 'image2image' : 'text2image', why: 'Cheaper A/B for the set and light.' },
    ],
    portrait: [
      { model: 'gpt-image-2', mode: refs.length ? 'image2image' : 'text2image', why: 'Hyperreal skin and fabric; holds identity from one reference.' },
      { model: 'wan2-7-image', mode: refs.length ? 'image2image' : 'text2image', why: 'Precise facial control at a flat price for alternates.' },
    ],
    series: [{ model: 'nano-banana-2-1', mode: 'image2image', why: 'Consistent person across scenes from one identity reference.' }],
    editorial: [{ model: 'nano-banana-2-1', mode: refs.length ? 'image2image' : 'text2image', why: 'Default for editorial photographs of people.' }],
  };
  const plan = plans[category] ?? plans.editorial ?? [];
  const first = plan[0];
  const aspect = category === 'product' ? '16:9' : category === 'poster' ? '2:3' : '4:5';
  const params: Record<string, unknown> =
    first?.model.startsWith('gpt')
      ? { aspectRatio: aspect, resolutionTier: category === 'product' ? '4k' : '2k', quality: 'high', imageCount: 2 }
      : first?.model === 'ideogram-4-5'
        ? { aspectRatio: aspect, resolution: '2k', quality: 'high', imageCount: 2 }
        : { aspectRatio: aspect, resolution: task === 'edit' ? '4K' : '2K', imageCount: task === 'edit' ? 8 : 4 };
  const labelled = refs.map((r, i) => ({ assetId: r.assetId, label: `image${i + 1}`, role: r.role ?? 'style' }));
  const refLine = labelled.length
    ? labelled.map((r) => `Image ${r.label.slice(5)} is the ${r.role === 'edit-source' ? 'photo to edit' : `${r.role} reference`}.`).join(' ') + ' '
    : '';
  const shot = {
    use: category === 'poster' ? 'Event poster, A2 print and social' : category === 'product' ? 'Campaign hero, web banner' : 'Editorial feature, print',
    format: { aspectRatio: aspect, resolution: String(params.resolution ?? params.resolutionTier ?? '2K'), ...(category === 'product' ? { copySpace: 'top left third' } : {}) },
    subject: firstSentence(brief) || 'As in the brief',
    setting: category === 'product' ? 'Wet slate on a dark set' : 'Quiet interior, uncluttered background',
    light: has(brief, /warm|dusk|golden/) ? 'Low warm side light, soft falloff' : 'Large soft window light from camera left, gentle fill',
    camera: category === 'product' ? '100 mm macro, f/8, eye level' : '85 mm, f/2.2, chest height',
    medium: 'Digital medium format, natural colour',
    retouch: 'Natural skin texture, pores visible; clean dust only',
    composition: category === 'product' ? 'Product on the lower third, copy space top left' : 'Rule of thirds, eyes on the upper third',
    moment: 'A candid beat between poses',
  };
  const prompt =
    task === 'edit'
      ? `${refLine}Relight and regrade image 1 to match the light and colour of image 2: ${shot.light.toLowerCase()}, muted teal shadows, warm highlights. Keep the person, pose, expression, wardrobe and framing of image 1 exactly. Natural skin texture, no smoothing.`
      : `${refLine}${shot.subject}. ${shot.setting}. ${shot.light}. Shot on ${shot.camera}. ${shot.medium}. ${shot.composition}. ${shot.retouch}.`;
  const questions = brief.length < 40 ? ['What is the picture for (cover, social, print)? It decides the format and crop.', 'Is there a person or product we must match? Add a reference.'] : undefined;
  return {
    category,
    spec: shot,
    plan,
    prompt,
    params,
    references: labelled,
    preserve: task === 'edit' ? ['face and identity', 'pose and expression', 'wardrobe', 'framing and crop'] : undefined,
    questions,
    notes: task === 'edit' ? 'Run all 8, then review against the source; retry only the ones with identity drift.' : undefined,
  };
}

function firstSentence(s: string): string {
  const m = /^[^.!?\n]{8,160}/.exec(s.trim());
  return (m?.[0] ?? s.slice(0, 120)).trim();
}

const DEFECTS: { area: string; problem: string; severity: 'minor' | 'major' }[] = [
  { area: 'hands', problem: 'Left hand has an ambiguous finger count at the cuff.', severity: 'major' },
  { area: 'skin', problem: 'Cheek slightly over-smoothed compared to the source.', severity: 'minor' },
  { area: 'light', problem: 'Catchlights point left while the key comes from the right.', severity: 'minor' },
  { area: 'background', problem: 'Repeating texture along the left edge.', severity: 'minor' },
  { area: 'identity', problem: 'Jawline narrower than in the source photo.', severity: 'major' },
  { area: 'wardrobe', problem: 'Collar changed shape from the source.', severity: 'minor' },
];

export function review(req: ReviewRequest): ReviewResponse {
  const r = rng(seedOf(req.assetId + (req.prompt ?? '')));
  const s = () => Math.min(5, Math.max(2, Math.round(3.2 + r() * 2)));
  const scores: ReviewResponse['scores'] = { realism: s(), skin: s(), light: s(), anatomy: s(), brief: s() };
  if (req.sourceAssetId) {
    scores.identity = s();
    scores.preserve = s();
  }
  const defects = DEFECTS.filter(() => r() < 0.22).slice(0, 3);
  if (scores.anatomy <= 3 && !defects.some((d) => d.area === 'hands')) defects.push(DEFECTS[0]!);
  const major = defects.some((d) => d.severity === 'major');
  const min = Math.min(...Object.values(scores).filter((x): x is number => typeof x === 'number'));
  const verdict: ReviewResponse['verdict'] = major ? (min <= 2 ? 'rethink' : 'retry') : min >= 4 ? 'accept' : 'retry';
  return {
    scores,
    defects,
    verdict,
    fix:
      verdict === 'accept'
        ? undefined
        : {
            prompt: `${req.prompt ? req.prompt.split('\n')[0] + '\n' : ''}${defects.map((d) => `Fix the ${d.area}: ${d.problem.toLowerCase().replace(/\.$/, '')}.`).join(' ') || 'Match the key light direction in the catchlights.'} Keep everything else unchanged.`,
            why: major ? 'A major defect would be visible in print.' : 'Small but visible at 100 %.',
          },
  };
}
