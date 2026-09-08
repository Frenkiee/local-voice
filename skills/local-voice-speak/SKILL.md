---
name: local-voice-speak
description: Narrate your work aloud with the local-voice MCP tools (speak_async by default) and use Supertonic 3 expression tags such as <laugh> correctly. Use whenever the local-voice MCP server is available and the user wants spoken progress updates.
---

# Speaking with local-voice

The `local-voice` MCP server exposes `speak_async` (fire and forget) and `speak`
(blocks ~1–2 s for synthesis, then plays in the background). Other apps' audio is
ducked to ~20 % while speech plays and fades back afterwards.

## When to speak

Use `mcp__local-voice__speak_async` for all narration. It returns immediately,
and consecutive calls are played in order under one duck. Reserve `speak` for
the rare case where the next step must not start before the audio is queued.

Speak at every milestone, one or two sentences each:

1. Before starting a task: what you are about to do.
2. When dispatching agents: how many and what for.
3. When an agent finishes: the one-line result.
4. When the task is done: what changed and what is next.
5. When the user must act: restart, rebuild, install, grant a permission.

Keep it short. Text is spoken verbatim, so no markdown, code, paths, or URLs.

## Expression tags (Supertonic 3 only)

Inline, lowercase, angle brackets, placed mid-sentence with no period right
before the tag:

```
The build passed <laugh> that was easier than expected.
<sigh> The deploy failed again, let me check the logs.
Take a moment <breath> and then run the tests.
```

- Verified audible: `<laugh>` (a real laugh), `<breath>`, `<sigh>`, `<cough>`.
- Interjection sounds the model renders rather than reads: `<hmm>`, `<mmm>`,
  `<uh>`, `<um>`, `<ah>`, `<oh>`.
- Anything else in angle brackets (`<whistle>`, `<pause>`, `<giggle>`,
  `<whisper>` …) is simply read aloud as a word. Do not use it.
- Square brackets do not work: `[laugh]` is read as the word "laugh".
- One tag per sentence at most. Tags after a period are weak because the
  sentence splitter starts a new chunk there.
- Speed 1.0 gives tags more room than 1.1. Wording and speed carry emotion
  more reliably than tags.

## Languages

Supertonic 3 speaks 31 languages. The language is a setting, not detected:
`set_config { language: "sl" }` via MCP or `local-voice config set language sl`.
Tags work in every language.
