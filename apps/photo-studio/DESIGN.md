# Photo Studio — design

A private, cloud-hosted node canvas for photographic image generation, in the spirit of Figma
Weave: every node is a step (brief, director, reference, generation, edit, review), wires carry
text and images between them, and a finished graph can be used through a simple form ("app
mode"). One owner. OpenArt is used only as the model provider; the method (how a brief becomes a
shot, which model gets which job, how references are labelled, how results are checked) is ours
and lives in the photo skill (`.claude/skills/photo-studio/`), which the server loads as the
director's knowledge.

Decisions (owner, 2026-10-15): React Flow front-end with a Rust server (an approved exception to
the repository's Rust-only rule, limited to `apps/photo-studio/web`); a single owner whose
OpenArt account is connected once from the settings page; the Anthropic API key lives on the
server.

## Layout

```
apps/photo-studio/
  Cargo.toml            Rust server (axum + tokio + reqwest), a workspace member
  src/                  server: auth, OpenArt OAuth + MCP client, jobs, assets, director, review
  web/                  React + TypeScript + Vite + @xyflow/react front-end (built to web/dist)
  DESIGN.md             this file
```

The server serves `web/dist` and the JSON API on one port. Data lives in one directory
(`--data DIR`, default `./photo-studio-data`):

```
data/
  secrets/openart.json     OAuth client registration + tokens (0600; never sent to the browser)
  assets/<id>.<ext>        full-resolution images (uploads and generations), copied from the CDN
  assets/<id>.thumb.jpg    512 px JPEG thumbnail
  assets/<id>.json         asset metadata
  jobs/<id>.json           generation jobs
  workflows/<id>.json      saved canvases
```

Writes are atomic (write to a temp file in the same directory, then rename).

## Configuration

| flag | env | meaning |
|---|---|---|
| `--host` / `--port` | `PHOTO_STUDIO_HOST` / `PHOTO_STUDIO_PORT` | bind address, default 127.0.0.1:8787 |
| `--data` | `PHOTO_STUDIO_DATA` | data directory |
| `--public-url` | `PHOTO_STUDIO_PUBLIC_URL` | external base URL, used for the OAuth redirect (`<url>/oauth/openart/callback`) and the `Secure` cookie flag |
| `--static` | `PHOTO_STUDIO_STATIC` | built front-end directory (default `web/dist` next to the binary's cwd) |
| `--skill` | `PHOTO_STUDIO_SKILL` | photo skill directory (default: the copy compiled into the binary) |
| — | `PHOTO_STUDIO_PASSWORD` | the owner's password (required; the server refuses to start without it) |
| — | `ANTHROPIC_API_KEY` | enables the director and review nodes |
| — | `PHOTO_STUDIO_CLAUDE_MODEL` | default `claude-opus-5-5` |

## Server responsibilities

1. **Owner auth.** `POST /api/login {password}` compares in constant time against
   `PHOTO_STUDIO_PASSWORD` and sets `ps_session` (random 256-bit, HttpOnly, SameSite=Strict,
   Secure when the public URL is https, 30 days). Sessions are kept in memory and in
   `data/secrets/sessions.json`. Five failed logins per minute per IP → 429. Every `/api/*` route
   except `/api/login` and `/healthz` requires the session; state-changing routes also require
   `Content-Type: application/json` (or multipart for uploads) as a CSRF guard.
2. **OpenArt connection** (OAuth 2.1, public client, PKCE S256, dynamic client registration),
   discovered from `https://mcp.openart.ai/.well-known/oauth-protected-resource/mcp` →
   authorization server `https://openart.ai` → `/.well-known/oauth-authorization-server`.
   Register once (persist the client), `GET /oauth/openart/start` redirects to the authorize
   endpoint with `scope=full_access`, `resource=https://mcp.openart.ai/mcp`, state and PKCE;
   `GET /oauth/openart/callback` exchanges the code. Access tokens refresh automatically with the
   refresh token; a refresh failure marks the connection as needing reconnect.
3. **MCP client** (Streamable HTTP, `https://mcp.openart.ai/mcp`): `initialize` →
   `notifications/initialized` → `tools/call`. Accept both `application/json` and
   `text/event-stream` responses; keep `Mcp-Session-Id`; re-initialize on 404/400 session errors;
   on 401 refresh the token once and retry. Tool results: parse `structuredContent`, else the
   first text block as JSON.
4. **Jobs.** `openart_generate_image` → `historyId`; a background task polls
   `openart_creation_get` honouring `pollAfterSeconds` (min 2 s, max 20 s, give up after 15 min);
   on COMPLETED, read the outputs (from the status payload or `openart_creation_list`), download
   every image into `assets/`, make thumbnails, and mark the job completed with its asset ids.
   Jobs survive restarts (resume polling for unfinished jobs on boot).
5. **Uploads.** `POST /api/assets` (multipart, field `file`, ≤ 30 MB, png/jpeg/webp) stores the
   file, then uploads it to OpenArt (`openart_upload_sign` → PUT → `openart_upload_metadata_get`)
   so it can be used as a reference; the asset keeps its OpenArt `{id, url}`. Generated assets
   already have one (the creation item id and CDN url).
6. **Director and review** (Anthropic Messages API, see `src/director.rs`): the system prompt is
   built from the photo skill files; responses are JSON validated against the shapes below.
   Review sends a ≤ 1568 px JPEG of the image (and of the source for edits).
7. **Static front-end** with SPA fallback to `index.html`.

## API (JSON, all under `/api`, session required unless noted)

```ts
type Me = {
  authenticated: boolean;
  openart: { connected: boolean; needsReconnect: boolean; email?: string; plan?: string; credits?: number };
  claude: { configured: boolean; model: string };
};
// GET  /api/me                       (no session needed; returns authenticated:false)
// POST /api/login   {password}       → Me
// POST /api/logout                   → {}
// GET  /oauth/openart/start          (browser redirect, session required)
// GET  /oauth/openart/callback       (browser redirect back to /?openart=connected|error)
// POST /api/openart/disconnect       → Me

type ModelMode = { mode: string; description: string; credits: number; affordable: boolean };
type Model = {
  id: string; name: string; description: string;
  modes: { image?: ModelMode[]; video?: ModelMode[] };
  categories: string[];          // from the skill's routing table, e.g. ["portrait","editorial"]
};
// GET  /api/models                   → { balance: number; models: Model[] }   (cached 10 min; ?refresh=1)
// GET  /api/models/:model/:mode/form → { schema: JSONSchema }                 (cached 1 h)
// POST /api/cost {model, mode, params} → { credits: number; config: object; affordable: boolean }

type Asset = {
  id: string; kind: "upload" | "generation";
  file: string; thumb: string;    // URLs served by us: /api/assets/:id/file, /api/assets/:id/thumb
  width: number; height: number; createdAt: number;
  openart?: { id: string; url: string };
  prompt?: string; model?: string; mode?: string; params?: object; jobId?: string;
};
// POST /api/assets (multipart: file)  → Asset
// GET  /api/assets?cursor=&limit=     → { assets: Asset[]; next?: string }   (newest first)
// GET  /api/assets/:id                → Asset
// GET  /api/assets/:id/file | /thumb  → image bytes (immutable cache headers)
// DELETE /api/assets/:id              → {}

type RefInput = { assetId: string; label?: string };   // labels default to image1, image2, … in order
type Job = {
  id: string; status: "queued" | "running" | "completed" | "failed";
  model: string; mode: string; params: object; references: RefInput[];
  historyId?: string; credits?: number; assets: Asset[]; error?: string;
  createdAt: number; updatedAt: number;
};
// POST /api/jobs {model, mode, params, references?: RefInput[]} → Job   (server sets params.visualReferences)
// GET  /api/jobs/:id                  → Job
// GET  /api/jobs?active=1             → { jobs: Job[] }

type ShotSpec = {
  use: string; format: { aspectRatio: string; resolution: string; copySpace?: string };
  subject: string; setting: string; light: string; camera: string; medium: string;
  retouch: string; composition: string; moment: string;
};
type DirectorRequest = {
  brief: string;                                    // any language
  references?: { assetId: string; role?: string }[];  // role hints: identity, outfit, product, lighting, style, pose, background, edit-source
  model?: string;                                   // force a model; otherwise the director routes
  task?: "generate" | "edit";                       // default: edit when an edit-source reference is given
};
type DirectorResponse = {
  category: string;                                 // routing category from the skill
  spec: ShotSpec;
  plan: { model: string; mode: string; why: string }[];   // first = recommended; >1 when chaining
  prompt: string;                                   // in the recommended model's dialect
  params: object;                                   // valid for that model's form (aspect, resolution, count…)
  references: { assetId: string; label: string; role: string }[];  // ordered as they must be sent
  preserve?: string[];                              // edit preserve list
  questions?: string[];                             // only when the brief is missing something decisive
  notes?: string;
};
// POST /api/director  DirectorRequest → DirectorResponse     (503 with {error} when Claude is not configured)

type ReviewRequest = { assetId: string; brief?: string; prompt?: string; sourceAssetId?: string; preserve?: string[] };
type ReviewResponse = {
  scores: { realism: number; skin: number; light: number; anatomy: number; brief: number; identity?: number; preserve?: number };  // 1–5
  defects: { area: string; problem: string; severity: "minor" | "major" }[];
  verdict: "accept" | "retry" | "rethink";
  fix?: { prompt: string; why: string };            // a corrected prompt or a targeted edit instruction
};
// POST /api/review  ReviewRequest → ReviewResponse

type Workflow = { id: string; name: string; graph: { nodes: unknown[]; edges: unknown[]; viewport?: unknown }; updatedAt: number };
// GET /api/workflows → { workflows: {id,name,updatedAt}[] } · POST /api/workflows {name, graph} → Workflow
// GET|PUT|DELETE /api/workflows/:id

// Errors: non-2xx with { error: string, code?: string }. Codes: unauthenticated, openart_not_connected,
// openart_reconnect, claude_not_configured, bad_request, upstream, not_found, rate_limited.
// GET /healthz → "ok" (no session)
```

## Front-end

React 19 + TypeScript + Vite, `@xyflow/react` for the canvas, Zustand for state, TanStack Query
for server state. No component kit is required; the look is our own (tokens in one CSS file,
dark-first with a light theme).

Node kinds (each a card with typed handles; wires coloured by type: text, image, spec):

| node | inputs | outputs | does |
|---|---|---|---|
| Brief | — | text | the client's words, any language |
| Reference | — | image | upload, pick from the library, or drop a file; a role chip (identity, outfit, product, lighting, style, pose, background, edit source) |
| Director | text, image[] | spec, text (prompt) | `POST /api/director`; shows category, plan (model chain), editable prompt, preserve list, questions |
| Generate | text, image[] (+ spec) | image[] | model picker with recommended models first, form rendered from the JSON Schema, live cost, count, Run; results grid inside the node |
| Edit | image (source), image[] refs, text | image[] | image2image with a "change only / keep exactly" builder |
| Review | image (+ source) | text (fix) | `POST /api/review`; scores, defects, verdict; "apply fix" creates a downstream Generate/Edit |
| Compare | image[] | image | grid of several branches; pick a winner |
| Output | image | — | full-size view, download, copy prompt, send to library |

Canvas: pan/zoom, minimap, snap, multi-select, copy/paste, undo/redo, drag images from the OS or
the library onto the canvas (creates Reference nodes), right-click menu, keyboard shortcuts.
Run one node or "Run to here" (topological run of upstream nodes); a running node shows progress;
outputs are cached on the node until its inputs change.

Templates (one click to a ready graph): Editorial edit from references (the owner's main use: a
source photo + mood reference → 8 variations at 4K → review), Portrait / headshot, Product hero,
Same person / new scene series, Text-on-image poster.

App mode: any workflow can be switched to a form that shows only the nodes marked "exposed"
(brief, references, a few parameters) and the Output, like Weave's Design App.

Panels: library (all assets, filter by kind/model), job queue, account (OpenArt balance, Claude
status), settings (connect/reconnect OpenArt).

## Deploy

One image, `ghcr.io/milkpack/photo-studio` (`packaging/photo-studio/Dockerfile`: the front-end
built with pnpm, the server with cargo, both in a Debian slim runtime), built and pushed by
`.github/workflows/photo-studio-image.yml` on every push that touches the app, the photo skill or
the packaging. On the VPS (Ubuntu 22.04/24.04, as root; it can share the server with VectorCraft
Cloud):

```sh
# first install: a password is generated and printed once at the end
curl -L raw.githubusercontent.com/milkpack/Claude/HEAD/go-studio | sudo ANTHROPIC_API_KEY=sk-ant-... bash
# same thing without the short entry point; options go after sudo
curl -fsSL https://raw.githubusercontent.com/milkpack/Claude/HEAD/packaging/photo-studio/deploy/install.sh \
  | sudo DOMAIN=photo.example.com ANTHROPIC_API_KEY=sk-ant-... bash
# update to the latest image (keeps the password, the key, the OpenArt connection and the data)
curl -L raw.githubusercontent.com/milkpack/Claude/HEAD/go-studio | sudo bash
```

`install.sh` puts everything in `/opt/photo-studio` (`docker-compose.yml`, `.env` with mode 0600,
`data/`, `backups/` with a daily tarball kept 14 days), runs the container on `127.0.0.1:8787`
with `--trust-proxy`, and serves it over HTTPS with the host's Caddy. The domain is `DOMAIN`, else
`photo.<ip>.sslip.io`; `PHOTO_STUDIO_PUBLIC_URL` is `https://<domain>`, and OpenArt sign-in only
works at that exact address. Without `ANTHROPIC_API_KEY` the director and review nodes are off
(run the installer again with the key to turn them on). Then open the site, sign in with the
password, and connect OpenArt in Settings → Connect OpenArt.

The Caddy site goes to `/etc/caddy/sites/photo-studio.caddy`; the main Caddyfile only gets an
`import /etc/caddy/sites/*.caddy` line, so a VectorCraft site in it is kept. The VectorCraft
installer rewrites the Caddyfile without that line: run the Photo Studio installer again afterwards
to restore it.

Locally: `docker build -f packaging/photo-studio/Dockerfile -t photo-studio .` and
`docker run -p 8787:8787 -v ps-data:/data -e PHOTO_STUDIO_PASSWORD='at least 12 chars' photo-studio`.
