// One-click starting graphs. Positions are laid out left to right in columns.
import type { AppEdge, AppNode } from './types';
import { makeEdge, makeNode } from './actions';

export type Template = {
  id: string;
  name: string;
  description: string;
  /** Short mono tag shown on the card (what it costs to run, roughly). */
  tag: string;
  build: () => { nodes: AppNode[]; edges: AppEdge[] };
};

const COL = 420;

export const TEMPLATES: Template[] = [
  {
    id: 'editorial-edit',
    name: 'Editorial edit from references',
    description: 'Source photo + mood reference → Edit, 8 variations at 4K → Review.',
    tag: '8 × 4K',
    build: () => {
      const source = makeNode('reference', { x: 0, y: 0 }, { title: 'Source photo', role: 'edit-source', exposed: true });
      const mood = makeNode('reference', { x: 0, y: 370 }, { title: 'Mood reference', role: 'lighting', exposed: true });
      const brief = makeNode('brief', { x: -20, y: 740 }, {
        title: 'Direction',
        exposed: true,
        text:
          'Editorial retouch for a magazine feature. Move the light and colour toward the mood reference: low warm side light, soft falloff, muted teal shadows. Natural skin texture, visible pores, no beauty filter.',
      });
      const edit = makeNode('edit', { x: COL, y: 120 }, {
        title: 'Edit · 8 variations',
        exposed: true,
        model: 'nano-banana-2-1',
        mode: 'image2image',
        category: 'editorial',
        params: { imageCount: 8, resolution: '4K', aspectRatio: '4:5' },
        change: ['light direction, quality and colour temperature from image 2', 'colour grade and shadow tone from image 2'],
        keep: ['face, identity and expression', 'pose and hands', 'wardrobe and hair', 'framing and crop'],
        prompt: '',
        runs: [],
      });
      const review = makeNode('review', { x: COL * 2 + 10, y: 0 }, {});
      const output = makeNode('output', { x: COL * 2 + 10, y: 520 }, { title: 'Selects' });
      return {
        nodes: [source, mood, brief, edit, review, output],
        edges: [
          makeEdge(source.id, 'image', edit.id, 'source', 'image'),
          makeEdge(mood.id, 'image', edit.id, 'refs', 'image'),
          makeEdge(brief.id, 'text', edit.id, 'prompt', 'text'),
          makeEdge(edit.id, 'images', review.id, 'image', 'image'),
          makeEdge(source.id, 'image', review.id, 'source', 'image'),
          makeEdge(brief.id, 'text', review.id, 'brief', 'text'),
          makeEdge(edit.id, 'images', output.id, 'image', 'image'),
        ],
      };
    },
  },
  {
    id: 'portrait',
    name: 'Portrait / headshot',
    description: 'Brief + identity reference → Director → Generate → Review.',
    tag: '4 × 2K',
    build: () => {
      const brief = makeNode('brief', { x: 0, y: 0 }, {
        exposed: true,
        text: 'Headshot for a law firm’s website. Mid-forties woman, navy blazer, calm and approachable. Soft window light, grey seamless, shallow depth of field.',
      });
      const person = makeNode('reference', { x: 20, y: 300 }, { title: 'Person', role: 'identity', exposed: true });
      const director = makeNode('director', { x: COL, y: 40 }, { task: 'generate' });
      const gen = makeNode('generate', { x: COL * 2 + 20, y: 0 }, {
        model: 'gpt-image-2',
        mode: 'image2image',
        category: 'portrait',
        params: { imageCount: 4, aspectRatio: '4:5', resolutionTier: '2k', quality: 'high' },
        prompt: '',
        runs: [],
        exposed: true,
      });
      const review = makeNode('review', { x: COL * 3 + 40, y: 0 }, {});
      const output = makeNode('output', { x: COL * 3 + 40, y: 520 }, {});
      return {
        nodes: [brief, person, director, gen, review, output],
        edges: [
          makeEdge(brief.id, 'text', director.id, 'brief', 'text'),
          makeEdge(person.id, 'image', director.id, 'refs', 'image'),
          makeEdge(director.id, 'spec', gen.id, 'spec', 'spec'),
          makeEdge(person.id, 'image', gen.id, 'refs', 'image'),
          makeEdge(gen.id, 'images', review.id, 'image', 'image'),
          makeEdge(brief.id, 'text', review.id, 'brief', 'text'),
          makeEdge(gen.id, 'images', output.id, 'image', 'image'),
        ],
      };
    },
  },
  {
    id: 'product-hero',
    name: 'Product hero',
    description: 'Packshot reference + brief → Director → highest-fidelity Generate.',
    tag: '2 × 4K',
    build: () => {
      const brief = makeNode('brief', { x: 0, y: 0 }, {
        exposed: true,
        text: 'Hero image for the autumn campaign: the amber glass bottle on wet slate, raking backlight, droplets, dark moody set. Leave copy space top left.',
      });
      const product = makeNode('reference', { x: 20, y: 300 }, { title: 'Packshot', role: 'product', exposed: true });
      const director = makeNode('director', { x: COL, y: 40 }, { task: 'generate' });
      const gen = makeNode('generate', { x: COL * 2 + 20, y: 0 }, {
        model: 'gpt-image-2-5-sunburst',
        mode: 'image2image',
        category: 'product',
        params: { imageCount: 2, aspectRatio: '16:9', resolutionTier: '4k', quality: 'high' },
        prompt: '',
        runs: [],
        exposed: true,
      });
      const output = makeNode('output', { x: COL * 3 + 40, y: 0 }, {});
      return {
        nodes: [brief, product, director, gen, output],
        edges: [
          makeEdge(brief.id, 'text', director.id, 'brief', 'text'),
          makeEdge(product.id, 'image', director.id, 'refs', 'image'),
          makeEdge(director.id, 'spec', gen.id, 'spec', 'spec'),
          makeEdge(product.id, 'image', gen.id, 'refs', 'image'),
          makeEdge(gen.id, 'images', output.id, 'image', 'image'),
        ],
      };
    },
  },
  {
    id: 'series',
    name: 'Same person, new scene series',
    description: 'One identity reference → four consistent scenes → Compare → pick.',
    tag: '4 × 2K',
    build: () => {
      const person = makeNode('reference', { x: 0, y: 0 }, { title: 'Person', role: 'identity', exposed: true });
      const brief = makeNode('brief', { x: -20, y: 380 }, {
        exposed: true,
        text: 'The same man from image 1 in four scenes of one morning: espresso at a zinc bar, crossing a rainy street, reading on a tram, at his studio desk. Same face, hair and coat throughout; 35 mm documentary look.',
      });
      const gen = makeNode('generate', { x: COL, y: 40 }, {
        model: 'nano-banana-2-1',
        mode: 'image2image',
        category: 'series',
        params: { imageCount: 4, aspectRatio: '4:5', resolution: '2K' },
        prompt: '',
        runs: [],
        exposed: true,
      });
      const compare = makeNode('compare', { x: COL * 2 + 20, y: 0 }, {});
      const output = makeNode('output', { x: COL * 3 + 40, y: 0 }, {});
      return {
        nodes: [person, brief, gen, compare, output],
        edges: [
          makeEdge(person.id, 'image', gen.id, 'refs', 'image'),
          makeEdge(brief.id, 'text', gen.id, 'prompt', 'text'),
          makeEdge(gen.id, 'images', compare.id, 'images', 'image'),
          makeEdge(compare.id, 'image', output.id, 'image', 'image'),
        ],
      };
    },
  },
  {
    id: 'poster',
    name: 'Text-on-image poster',
    description: 'Brief with exact wording → Director → typography model → Review.',
    tag: '2 × 2K',
    build: () => {
      const brief = makeNode('brief', { x: 0, y: 0 }, {
        exposed: true,
        text: 'Poster for a jazz night. Exact text: “BLUE HOUR SESSIONS” as the headline, “Fridays · 21:00 · Hall 4” below. Photographic: a saxophonist in silhouette against a dusk window, grainy, cool blue palette.',
      });
      const director = makeNode('director', { x: COL - 20, y: 0 }, { task: 'generate' });
      const gen = makeNode('generate', { x: COL * 2, y: 0 }, {
        model: 'ideogram-4-5',
        mode: 'text2image',
        category: 'poster',
        params: { imageCount: 2, aspectRatio: '2:3', resolution: '2k', quality: 'high' },
        prompt: '',
        runs: [],
        exposed: true,
      });
      const review = makeNode('review', { x: COL * 3 + 20, y: 0 }, {});
      const output = makeNode('output', { x: COL * 3 + 20, y: 520 }, {});
      return {
        nodes: [brief, director, gen, review, output],
        edges: [
          makeEdge(brief.id, 'text', director.id, 'brief', 'text'),
          makeEdge(director.id, 'spec', gen.id, 'spec', 'spec'),
          makeEdge(gen.id, 'images', review.id, 'image', 'image'),
          makeEdge(brief.id, 'text', review.id, 'brief', 'text'),
          makeEdge(gen.id, 'images', output.id, 'image', 'image'),
        ],
      };
    },
  },
];
