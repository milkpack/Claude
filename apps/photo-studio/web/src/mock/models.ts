// The mock model catalogue, shaped like the server's /api/models (from the photo skill's
// models.md snapshot and real openart_model_form_get schemas).
import type { JSONSchema } from '../api/types';

type Family = 'nano' | 'nano-lite' | 'gpt' | 'wan' | 'kling' | 'grok' | 'seedream' | 'seedream-pro' | 'ideogram';

export type MockModel = {
  id: string;
  name: string;
  description: string;
  family: Family;
  categories: string[];
  t2i: number;
  i2i: number;
  maxRefs: number;
  /** Only image2image. */
  i2iOnly?: boolean;
};

export const MOCK_MODELS: MockModel[] = [
  { id: 'nano-banana-2-1', name: 'Nano Banana 2.1', description: 'Default for photos: people, editorial, multi-subject.', family: 'nano', categories: ['editorial', 'portrait', 'edit', 'series'], t2i: 30, i2i: 32, maxRefs: 14 },
  { id: 'gpt-image-2-5-sunburst', name: 'GPT Image 2.5 Sunburst', description: 'Highest quality and the most precise editing.', family: 'gpt', categories: ['product', 'edit', 'portrait'], t2i: 40, i2i: 42, maxRefs: 16 },
  { id: 'gpt-image-2', name: 'GPT Image 2', description: 'Hyperreal people and products, luxury, tiny text.', family: 'gpt', categories: ['portrait', 'product'], t2i: 40, i2i: 42, maxRefs: 16 },
  { id: 'gpt-image-2-5-flare', name: 'GPT Image 2.5 Flare', description: 'Newest GPT Image, faster.', family: 'gpt', categories: ['product'], t2i: 40, i2i: 42, maxRefs: 16 },
  { id: 'nano-banana-pro', name: 'Nano Banana Pro', description: 'Long in-image text, multi-turn edits, up to 5 consistent people.', family: 'nano', categories: ['poster', 'series', 'editorial'], t2i: 40, i2i: 40, maxRefs: 14 },
  { id: 'nano-banana-2', name: 'Nano Banana 2', description: 'Same family as 2.1, cheaper.', family: 'nano', categories: ['editorial', 'draft'], t2i: 20, i2i: 20, maxRefs: 14 },
  { id: 'nano-banana-2-lite', name: 'Nano Banana 2 Lite', description: 'Fast 1K drafts; no character slots.', family: 'nano-lite', categories: ['draft'], t2i: 15, i2i: 15, maxRefs: 10 },
  { id: 'wan2-7-image', name: 'Wan 2.7 Image', description: 'Precise facial control and in-image text at a flat price.', family: 'wan', categories: ['portrait', 'poster'], t2i: 15, i2i: 15, maxRefs: 9 },
  { id: 'kling-3-omni', name: 'Kling 3 Omni', description: 'Cheap 4K and coherent series of 2–9 frames.', family: 'kling', categories: ['series', 'draft'], t2i: 10, i2i: 10, maxRefs: 10 },
  { id: 'grok-imagine-image-2', name: 'Grok Imagine Image 2', description: 'Editing an existing photo: region edits, background removal, reframing.', family: 'grok', categories: ['edit'], t2i: 26, i2i: 33, maxRefs: 3 },
  { id: 'ideogram-4-5', name: 'Ideogram 4.5', description: 'Typography, posters, signage and logos.', family: 'ideogram', categories: ['poster'], t2i: 44, i2i: 74, maxRefs: 5 },
  { id: 'byte-plus-seedream-4-5', name: 'Seedream 4.5', description: 'Stylized aesthetics; not the first choice for realism.', family: 'seedream', categories: ['stylized'], t2i: 15, i2i: 15, maxRefs: 14 },
  { id: 'byte-plus-seedream-5-pro', name: 'Seedream 5 Pro', description: 'Premium stylized.', family: 'seedream-pro', categories: ['stylized'], t2i: 30, i2i: 30, maxRefs: 10 },
];

const str = (values: string[], def: string): JSONSchema => ({ type: 'string', enum: values, default: def });
const count: JSONSchema = { type: 'integer', minimum: 1, maximum: 8, default: 1 };
const prompt = (max?: number): JSONSchema => ({ type: 'string', minLength: 1, ...(max ? { maxLength: max } : {}) });
const refs = (max: number): JSONSchema => ({
  type: 'array',
  minItems: 1,
  maxItems: max,
  items: { type: 'object', properties: { type: { type: 'string', const: 'image' }, id: { type: 'string' }, url: { type: 'string' }, label: { type: 'string' } } },
});

export function schemaFor(m: MockModel, mode: string): JSONSchema {
  const p: Record<string, JSONSchema> = { prompt: prompt(m.family === 'kling' ? 2500 : m.family === 'wan' ? 5000 : undefined), imageCount: count };
  switch (m.family) {
    case 'nano':
      p.aspectRatio = str(['21:9', '16:9', '3:2', '4:3', '5:4', '1:1', '4:5', '3:4', '2:3', '9:16'], '1:1');
      p.resolution = str(['1K', '2K', '4K'], '1K');
      break;
    case 'nano-lite':
      p.aspectRatio = str(['21:9', '16:9', '3:2', '4:3', '5:4', '1:1', '4:5', '3:4', '2:3', '9:16'], '1:1');
      break;
    case 'gpt':
      p.aspectRatio = str(['1:1', '3:2', '2:3', '16:9', '9:16', '4:3', '3:4', '5:4', '4:5', '21:9', '9:21'], '4:3');
      p.resolutionTier = str(['1k', '2k', '4k'], '2k');
      p.quality = str(m.id.includes('sunburst') ? ['low', 'medium', 'high'] : ['low', 'medium', 'high', 'auto'], 'medium');
      p.lockAspectRatio = { type: 'boolean', default: true };
      p.customWidth = { type: 'integer', exclusiveMinimum: 0, maximum: 8192 };
      p.customHeight = { type: 'integer', exclusiveMinimum: 0, maximum: 8192 };
      p.outputFormat = str(['png', 'jpeg', 'webp'], 'png');
      p.outputCompression = { type: 'integer', minimum: 0, maximum: 100 };
      if (m.id.includes('sunburst')) p.variant = { type: 'string', const: 'sunburst' };
      break;
    case 'wan':
      p.aspectRatio = str(['1:1', '4:3', '3:4', '16:9', '9:16', '3:2', '2:3', '21:9', '9:21'], '1:1');
      p.resolution = str(['2K', '4K'], '4K');
      p.mode = str(['pro', 'standard'], 'pro');
      p.thinkingMode = { type: 'boolean', default: false };
      break;
    case 'kling':
      p.resolution = str(['1k', '2k', '4k'], '1k');
      p.resultType = str(['single', 'series'], 'single');
      p.seriesAmount = { type: 'integer', minimum: 2, maximum: 9, default: 4 };
      p.aspectRatio = str(['16:9', '9:16', '1:1', '4:3', '3:4', '3:2', '2:3', '21:9', 'auto'], '4:3');
      break;
    case 'grok':
      p.aspectRatio = str(['auto', '1:1', '4:3', '3:4', '16:9', '9:16', '3:2', '2:3', '2:1', '19.5:9', '9:21'], 'auto');
      p.resolution = str(['1K', '2K'], '1K');
      p.quality = str(['low', 'medium'], 'low');
      break;
    case 'seedream':
      p.aspectRatio = str(['1:1', '4:3', '3:4', '16:9', '9:16', '3:2', '2:3', '21:9'], '1:1');
      p.resolution = str(['2K', '4K'], '2K');
      break;
    case 'seedream-pro':
      p.aspectRatio = str(['1:1', '4:3', '3:4', '16:9', '9:16', '3:2', '2:3', '21:9'], '1:1');
      p.resolution = str(['1K', '2K'], '1K');
      break;
    case 'ideogram':
      p.quality = str(['very_low', 'low', 'medium', 'high'], 'medium');
      p.resolution = str(['1k', '2k'], '1k');
      p.aspectRatio = str(['auto', '1:1', '4:3', '3:4', '16:9', '9:16', '3:2', '2:3', '4:5', '5:4'], 'auto');
      break;
  }
  p.autoEnhancePrompt = { type: 'boolean', default: false };
  const required = ['prompt'];
  if (mode === 'image2image') {
    p.visualReferences = refs(m.maxRefs);
    required.push('visualReferences');
  }
  return { type: 'object', properties: p, required, additionalProperties: false };
}

export function modesOf(m: MockModel): string[] {
  return m.i2iOnly ? ['image2image'] : ['text2image', 'image2image'];
}

/** Credits for one job (unit × count), mirroring the shape of openart_model_cost. */
export function priceOf(m: MockModel, mode: string, params: Record<string, unknown>): { credits: number; config: Record<string, unknown> } {
  const schema = schemaFor(m, mode);
  const props = schema.properties ?? {};
  const config: Record<string, unknown> = {};
  for (const [k, s] of Object.entries(props)) {
    if (k === 'prompt' || k === 'visualReferences' || k === 'autoEnhancePrompt' || s.const !== undefined) continue;
    const v = params[k] ?? s.default;
    if (v !== undefined) config[k] = v;
  }
  let unit = mode === 'image2image' ? m.i2i : m.t2i;
  const res = String(config.resolution ?? config.resolutionTier ?? '').toUpperCase();
  const quality = String(config.quality ?? '');
  switch (m.family) {
    case 'nano':
      if (res === '2K') unit = Math.round(unit * 1.5);
      if (res === '4K') unit = 80;
      break;
    case 'gpt': {
      const q = quality === 'high' ? 2.6 : quality === 'low' ? 0.4 : 1;
      const r = res === '4K' ? 2 : res === '1K' ? 0.6 : 1;
      const ar = String(config.aspectRatio ?? '1:1');
      const wide = ar === '16:9' || ar === '9:16' || ar === '21:9' || ar === '9:21' ? 1.25 : ar === '1:1' ? 0.84 : 1;
      unit = Math.round(unit * q * r * (q > 2 && r > 1 ? wide : 1));
      break;
    }
    case 'wan':
      if (config.mode === 'standard') unit = 6;
      break;
    case 'kling':
      if (res === '4K') unit = 20;
      else if (res === '2K') unit = 14;
      break;
    case 'grok':
      if (quality === 'medium') unit += 10;
      if (res === '2K') unit += 8;
      break;
    case 'seedream':
      if (res === '4K') unit = 24;
      break;
    case 'seedream-pro':
      if (res === '2K') unit *= 2;
      break;
    case 'ideogram':
      unit = quality === 'high' ? (mode === 'image2image' ? 160 : 74) : quality === 'low' ? 24 : quality === 'very_low' ? 12 : unit;
      break;
    default:
      break;
  }
  const n = config.resultType === 'series' ? Number(config.seriesAmount ?? 4) : Number(config.imageCount ?? 1);
  const qty = Number.isFinite(n) ? Math.min(9, Math.max(1, Math.round(n))) : 1;
  return { credits: unit * qty, config };
}

/** Pixel size the "provider" would return, from aspect ratio and resolution. */
export function outputSize(params: Record<string, unknown>): { width: number; height: number } {
  const res = String(params.resolution ?? params.resolutionTier ?? '1K').toUpperCase();
  const long = res === '4K' ? 4096 : res === '2K' ? 2048 : 1024;
  const ar = String(params.aspectRatio ?? '1:1');
  const m = /^(\d+(?:\.\d+)?):(\d+(?:\.\d+)?)$/.exec(ar);
  const a = m ? Number(m[1]) / Number(m[2]) : 1;
  const ratio = Number.isFinite(a) && a > 0 ? a : 1;
  return ratio >= 1 ? { width: long, height: Math.round(long / ratio) } : { width: Math.round(long * ratio), height: long };
}
