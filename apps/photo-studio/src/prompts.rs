//! Handoff prompts: the text the owner pastes into Claude for a task, written from per-kind
//! templates and the photo skill (no LLM). Each prompt carries the brief and node settings,
//! every input as a numbered link with its role, the instructions for the kind, the exact return
//! format, and `curl` lines (absolute URLs from `--public-url`) for downloading the inputs and
//! returning the result, so Claude Code can do the round trip by itself.

use serde_json::Value;

use crate::server::App;
use crate::tasks::{StoredTask, TaskKind, input_url, return_url};

/// `YYYY-MM-DD HH:MM UTC` for milliseconds since the epoch.
pub fn utc(ms: u64) -> String {
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let (h, m) = ((secs % 86_400) / 3600, (secs % 3600) / 60);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02} UTC")
}

fn file_ext(app: &App, asset_id: &str) -> String {
    app.assets.get(asset_id).map(|a| a.ext).unwrap_or_else(|| "png".into())
}

fn intro(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Director => {
            "You are the director for Photo Studio, the owner's node canvas for photographic image generation on OpenArt. \
Turn the brief below into a shot and a ready-to-run generation plan, following the photo skill (attached at the end; \
use your installed copy if you have one). You don't generate anything here: the Generate node quotes and runs your plan."
        }
        TaskKind::Review => {
            "You are the reviewer for Photo Studio. Check the generated photograph against the brief and the prompt that made \
it (and, for an edit, against the source image), following the photo skill's review checklist (attached at the end)."
        }
        TaskKind::Blender => {
            "You are working in Blender for Photo Studio. Build the scene described below and render it, so the renders can be \
used as references (composition, light, camera) for a photographic generation."
        }
        TaskKind::Photoshop => "You are retouching in Photoshop for Photo Studio. Make one local change to an existing photograph and nothing else.",
        TaskKind::Sketch => {
            "You are the photo editor for Photo Studio. Look at the image as you would before sending it to a client, following \
the photo skill's review checklist (attached at the end), and mark every issue on it."
        }
        TaskKind::Custom => "This is a task from Photo Studio, the owner's node canvas for photographic image generation.",
    }
}

fn what_to_do(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Director => {
            "Do the skill's workflow steps 1 to 5: read the brief, decide the shot, route it to a model, plan the references \
(label them in the order the prompt names them), and write the prompt in that model's dialect with its params. \
Look at every input image before you label it."
        }
        TaskKind::Review => {
            "Input 1 is the image under review. An input with the role `source` (or `edit-source`) is the image before the \
edit. Look at the full frame, then at faces, hands and any text up close. Score, list the defects, give a verdict, and \
write a fix (a corrected prompt or a targeted edit instruction) when the verdict isn't accept."
        }
        TaskKind::Blender => {
            "1. Build the scene in Blender (through a Blender MCP server, or a Python script run with \
`blender --background --python scene.py`): subject stand-ins, set, camera, lens and lights as described.\n\
2. Render each requested view as a PNG (if no views are named: one hero view at the stated aspect ratio, 2048 px on the \
long side).\n\
3. Return the renders, a one-paragraph note in `text` (camera position, focal length, lights and their sizes/colours), and \
the .blend file only if the instructions ask for it."
        }
        TaskKind::Photoshop => {
            "1. Open input 1 (the source). If an input has the role `mask`, its white area is the only region you may change.\n\
2. Make only the change described in the instructions: a selection from the mask or the named region, then retouch, \
generative/content-aware fill or an adjustment layer, whichever does it cleanly.\n\
3. Keep every other pixel identical: same size, no crop, no resampling, no global colour or tone change.\n\
4. Export a PNG at the original size (and the PSD with layers only if the instructions ask for it) and return it."
        }
        TaskKind::Sketch => {
            "1. Check input 1 with the review checklist: realism, skin, light, anatomy, text, and the brief.\n\
2. On a copy at the original size, mark each issue: a circle or an arrow and a number next to it. Keep marks thin and \
high-contrast; don't cover the defect itself.\n\
3. Return the annotated PNG, and in `text` the numbered list of issues: `n. area: problem (minor|major), fix`."
        }
        TaskKind::Custom => "Follow the instructions below.",
    }
}

fn return_format(kind: TaskKind) -> String {
    match kind {
        TaskKind::Director => format!(
            "Your answer is exactly one fenced ```json block with a `DirectorResponse` object (a short note before it is fine; \
pasting the block into the Director node fills its plan, prompt, params and preserve list). Use the input asset ids listed \
above as `assetId`.\n\n{}",
            crate::director::CONTRACT
        ),
        TaskKind::Review => format!(
            "Your answer is exactly one fenced ```json block with a `ReviewResponse` object (a short note before it is fine).\n\n{}",
            crate::review::CONTRACT
        ),
        TaskKind::Blender => "Return the PNG renders as files and the note as `text` (and the .blend if asked).".into(),
        TaskKind::Photoshop => "Return the edited PNG as a file (and the PSD if asked); in `text`, one line on what you changed.".into(),
        TaskKind::Sketch => "Return the annotated PNG as a file and the numbered list of issues as `text`.".into(),
        TaskKind::Custom => "Return your answer as `text`, and any files you make.".into(),
    }
}

fn wants_skill(kind: TaskKind) -> bool {
    matches!(kind, TaskKind::Director | TaskKind::Review | TaskKind::Sketch)
}

/// The full prompt of `task`.
pub fn build(app: &App, task: &StoredTask, instructions: &str, context: Option<&Value>, models: Option<&[Value]>) -> String {
    let mut p = String::new();
    p.push_str(&format!("# {}\n\n{}\n\n", task.title, intro(task.kind)));

    p.push_str("## Brief and instructions\n\n");
    if instructions.trim().is_empty() {
        p.push_str("(none given: work from the inputs and the node settings)\n\n");
    } else {
        p.push_str(instructions.trim());
        p.push_str("\n\n");
    }
    if let Some(c) = context {
        p.push_str("## Node settings\n\n```json\n");
        p.push_str(&serde_json::to_string_pretty(c).unwrap_or_default());
        p.push_str("\n```\n\n");
    }

    p.push_str("## Inputs\n\n");
    if task.inputs.is_empty() {
        p.push_str("(no input images)\n\n");
    }
    for (i, inp) in task.inputs.iter().enumerate() {
        let n = i + 1;
        let role = inp.role.as_deref().unwrap_or("reference");
        let meta = app
            .assets
            .get(&inp.asset_id)
            .map(|a| {
                let made = a.model.as_ref().map(|m| format!(", generated with {m}")).unwrap_or_default();
                format!("{}×{} {}{made}", a.width, a.height, a.ext)
            })
            .unwrap_or_default();
        p.push_str(&format!("{n}. **image {n}**, role: {role}, assetId `{}` ({meta}): {}\n", inp.asset_id, input_url(app, task, &inp.asset_id)));
    }
    if !task.inputs.is_empty() {
        p.push('\n');
    }

    if let Some(list) = models {
        p.push_str("## Available OpenArt image models (live; credits per job at default settings)\n\n```json\n");
        p.push_str(&serde_json::to_string(list).unwrap_or_default());
        p.push_str("\n```\n\n");
    }

    p.push_str(&format!("## What to do\n\n{}\n\n", what_to_do(task.kind)));
    p.push_str(&format!("## Return format\n\n{}\n\n", return_format(task.kind)));

    let ret = return_url(app, task);
    p.push_str("## Download the inputs\n\n");
    if task.inputs.is_empty() {
        p.push_str("(nothing to download)\n\n");
    } else {
        p.push_str("```sh\n");
        for (i, inp) in task.inputs.iter().enumerate() {
            p.push_str(&format!("curl -fsSL -o input{}.{} '{}'\n", i + 1, file_ext(app, &inp.asset_id), input_url(app, task, &inp.asset_id)));
        }
        p.push_str("```\n\n");
    }
    p.push_str("## Return the result\n\n");
    let example = match task.kind {
        TaskKind::Director | TaskKind::Review => format!("curl -fsS -F 'text=<answer.md' '{ret}'"),
        TaskKind::Custom => format!("curl -fsS -F 'text=<answer.md' -F 'files=@out.png' '{ret}'"),
        TaskKind::Blender => format!("curl -fsS -F 'text=<note.md' -F 'files=@render1.png' -F 'files=@render2.png' '{ret}'"),
        TaskKind::Photoshop => format!("curl -fsS -F 'text=<note.md' -F 'files=@edited.png' '{ret}'"),
        TaskKind::Sketch => format!("curl -fsS -F 'text=<issues.md' -F 'files=@annotated.png' '{ret}'"),
    };
    p.push_str(&format!(
        "Write your answer to a file and send it, with any files, to the return link:\n\n```sh\n{example}\n```\n\n\
`text=<file` makes curl send that file's contents; add one `-F 'files=@…'` per file (at most {} files of {} MB). \
A person can open {ret} in a browser and use the form instead. The links need no sign-in and work until {}.\n",
        crate::tasks::MAX_RESULT_FILES,
        crate::tasks::MAX_RESULT_FILE >> 20,
        utc(task.expires_at)
    ));

    if wants_skill(task.kind) && !app.skill_text.is_empty() {
        p.push_str("\n---\n\n# The photo skill\n\n");
        p.push_str(&app.skill_text);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1_792_108_800_000), "2026-10-16 00:00 UTC");
        assert_eq!(utc(951_782_400_000 + 3_600_000 + 60_000), "2000-02-29 01:01 UTC");
    }
}
