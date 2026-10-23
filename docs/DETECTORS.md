# Detectors

Recap finds plays by **looking at the screen**, the same way you do. Every game's
detectors are plain data files, so anyone can add a game or fix a detector without
touching code, and share it as a folder.

Detection runs **after the match**, over the finished recording. It costs nothing
while you play, and it never touches the game process: Recap only looks at pixels
the game already put on screen.

Games with an official *local* API (League of Legends' Live Client Data API, CS2's
Game State Integration) can use it as an extra event source. API events and vision
events are the same `GameEvent` type, so the timeline, auto-clips and library treat
them identically.

---

## How a detector works

1. **Sample.** The recording is decoded at the detector's `sampleFps` (e.g. 6 frames
   per second), scaled to the analysis height (720p by default), and converted to grayscale.
2. **Crop.** Only the detector's `region` is examined. The region is stored as
   **fractions of the frame** (`x`, `y`, `w`, `h` between 0 and 1), so the same
   file works at 1080p, 1440p or 4K.
3. **Match.** Each reference image (a *template*) slides across the region and is
   scored with **zero-mean normalized cross-correlation (ZNCC)**. A score of 1.0
   means the pixels match exactly up to brightness and contrast; unrelated content
   scores near 0. Because the mean is subtracted, a dimmer or brighter HUD still matches.
4. **Rescale.** Templates are cut at some resolution, recorded as
   `templateFrameHeight`. Before matching, each template is rescaled by
   `analysisHeight / templateFrameHeight`, so a template cut from a 1440p frame
   still works on a 1080p recording. `scales` adds extra sizes to try (e.g.
   `[0.9, 1.1]`) for people who changed their HUD scale.
5. **Trigger.** A detector fires when the score is at or above `threshold` for
   `minConsecutive` samples in a row (this filters single-frame flickers). It fires
   once per appearance, at the time the indicator first appeared, and never more
   often than `cooldownMs`.

Work is kept small: templates are downscaled so their longest side is at most 96 px
before matching. A typical detector costs well under a millisecond per sample.

## File layout

```text
games/valorant/
  game.json                 # the game: process names, capture mode, API, auto-clip events
  detectors/
    kill-banner.json        # one detector per file
    death-recap.json
  templates/
    kill-banner-1.png       # reference images, referenced by detectors
```

### `game.json`

```json
{
  "id": "valorant",
  "name": "VALORANT",
  "processes": ["VALORANT-Win64-Shipping.exe"],
  "capture": "game",
  "api": null,
  "preferApi": true,
  "autoClip": { "events": ["kill"] },
  "postMatch": [],
  "color": "#FF4655"
}
```

| Key | Meaning |
|---|---|
| `id` | Folder-safe id (letters, digits, `-`, `_`). |
| `processes` | Executable names, case-insensitive. When one is running, Recap records. |
| `capture` | `game` (OBS game hook, best), `window`, or `monitor`. |
| `api` | Optional: `league-live-client` or `cs2-gsi`. |
| `preferApi` | When an API is available, skip vision detectors for event kinds the API already reports. |
| `autoClip.events` | Event kinds that get turned into clips. |
| `postMatch` | Optional post-match processors (see below). |
| `color` | Accent colour used in the UI. |

### `detectors/<id>.json`

```json
{
  "id": "kill-banner",
  "label": "Kill banner",
  "event": "kill",
  "region": { "x": 0.42, "y": 0.66, "w": 0.16, "h": 0.16 },
  "templateBox": { "x": 0.46, "y": 0.70, "w": 0.08, "h": 0.08 },
  "templates": ["templates/kill-banner-1.png"],
  "templateFrameHeight": 1080,
  "method": "ncc",
  "threshold": 0.8,
  "cooldownMs": 600,
  "minConsecutive": 2,
  "sampleFps": 8,
  "scales": [0.9, 1.1],
  "enabled": true,
  "notes": "Captured on default HUD scale."
}
```

| Key | Default | Meaning |
|---|---|---|
| `event` | – | Event kind to emit: `kill`, `death`, `assist`, `round_win`, … or any custom string. |
| `region` | – | Where to search, as frame fractions. Make it a little larger than the template (Studio adds a margin for you). |
| `templateBox` | – | Where the template was cut from (display only). |
| `templates` | `[]` | Reference PNGs, relative to the game folder. Several templates = any of them can match. A detector with no templates is listed but does not run. |
| `templateFrameHeight` | – | Height of the frame the templates were cut from. |
| `threshold` | `0.8` | Minimum ZNCC score. Raise it if you get false hits, lower it if you miss real ones. |
| `cooldownMs` | `1500` | Minimum time between two events from this detector. |
| `minConsecutive` | `2` | Matching samples in a row needed to fire. |
| `sampleFps` | `6` | Samples per second. The indicator must stay on screen for at least `minConsecutive / sampleFps` seconds. |
| `scales` | `[]` | Extra template scales to try; `1.0` is always tried. |

### Where files live, and overrides

* **Bundled** games ship inside the app (`<install dir>/games/`).
* **Your** games and detectors live in `%APPDATA%\com.recap.app\games\<game-id>\`.

Recap loads the bundled folder first, then your folder. If your folder contains a
`game.json` for the same `id`, it replaces the bundled one. Detectors are merged
**one by one**: a detector in your folder with the same `id` replaces the bundled
detector, and any new ids are added. Anything Detector Studio saves goes to your
folder, so app updates never overwrite your work.

## Making a detector in Detector Studio

1. Play a match with Recap running (or open any recording in the library).
2. Open **Detector Studio**, pick the game, and choose **New detector**. Name it
   and choose the event it represents (e.g. `kill`).
3. Pick a source: a recording, or an image file (a screenshot works too).
4. Scrub to a frame where the indicator is clearly visible, e.g. the moment the
   kill banner pops.
5. **Drag a box tightly around the indicator.** Include the distinctive part
   (icon, shape) and avoid things that change between appearances (player names,
   numbers, the game world behind it). Studio saves the crop as a template,
   records the frame height, and sets the search region to the box plus a margin.
6. **Test** the detector against a whole recording. Studio plots the score over
   time with your threshold line and lists every hit. Click a hit to jump to it.
7. Tune it:
   * missed real events → lower `threshold` a little, or add a second template
     from another appearance (different agent, map lighting or multi-kill variant);
   * false hits → raise `threshold`, tighten the box, or raise `minConsecutive`;
   * one event counted twice → raise `cooldownMs`.
8. **Save.** Use **Rescan** on older recordings to apply it to them too.

Tips:

* Prefer **your** indicators (a personal kill banner, a hit marker, a "you died"
  panel) over the shared kill feed, which shows everyone's kills.
* Kill-cams, death recaps and spectating can put kill visuals on screen that are
  not yours. A death detector plus a short `cooldownMs` window usually helps you
  spot and delete those, and a tight region avoids most of them.
* Colourblind modes and HUD colour settings change what the indicator looks like.
  Matching is done in grayscale and is contrast-invariant, but shapes still have to match.

## Sharing detectors

A game folder is self-contained. To share a detector, zip
`%APPDATA%\com.recap.app\games\<game-id>\` (or just `detectors/<id>.json` plus the
templates it references) and have the other person unzip it into the same place.
Templates are crops of your own screen. Keep them small and tight around the HUD
element, which is also what makes them match best.

To contribute a detector to the bundled set, open a pull request adding the files
under `games/<game-id>/`.

## Other event sources

### Official local APIs

Set `"api"` in `game.json`:

* `league-live-client`: polls `https://127.0.0.1:2999/liveclientdata/allgamedata`
  while you play (kills, deaths, assists, multikills, aces, objectives). This works for
  League of Legends. TFT runs in the same process, so it shares the League entry.
* `cs2-gsi`: Recap installs `gamestate_integration_recap.cfg` into your CS2 `cfg`
  folder and receives Valve's Game State Integration updates on `127.0.0.1` (kills,
  deaths, round wins and losses, MVPs, bomb plants and defuses).

Both are part of the games themselves, run entirely on your PC, and need no account.

### Post-match processors

Some games write their own replay or demo files. A post-match processor runs after
the match and turns those files into events. Processors are external commands
declared in `game.json`:

```json
"postMatch": [
  {
    "type": "command",
    "id": "my-replay-parser",
    "command": "C:\\Tools\\my-replay-parser.exe",
    "args": ["--dir", "{gameDir}", "--from", "{matchStartUnixMs}", "--to", "{matchEndUnixMs}"],
    "watchDir": "%LOCALAPPDATA%\\SomeGame\\Replays"
  }
]
```

Placeholders substituted in `args`: `{recording}` (the .mp4 path),
`{matchStartUnixMs}`, `{matchEndUnixMs}`, `{gameDir}`.

The command must print a JSON array of events to stdout:

```json
[
  { "kind": "kill", "tMs": 83500, "meta": { "victim": "PlayerTwo" } },
  { "kind": "death", "unixMs": 1761258000123 }
]
```

Each event gives **either** `tMs` (milliseconds from the start of the recording)
**or** `unixMs` (wall-clock time; Recap converts it using the recording start time).
`meta` is optional and shown in the event's details. A failing processor is logged
and skipped; it never blocks the recording from being saved.

Tools that already parse a game's replays (for example community R6 Siege `.rec`
parsers) can be wrapped with a small script that prints this format.
