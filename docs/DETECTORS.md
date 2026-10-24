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
| `autoClip.timing` | Optional seconds around each kind, e.g. `{ "kill": { "before": 6, "after": 3 } }`. Overrides Recap's built-in per-kind timing; the user's own timing in Settings beats both. An event with `meta.endMs` (a multikill) is clipped from its start to its end. |
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
| `templates` | `[]` | Reference PNGs, relative to the game folder. Several templates = any of them can match. A detector with no templates is listed but does not run. References of the same element (cut with the same box) also teach Recap which pixels belong to it; see below. |
| `templateFrameHeight` | – | Height of the frame the templates were cut from. |
| `threshold` | `0.8` | Minimum ZNCC score. Raise it if you get false hits, lower it if you miss real ones. |
| `cooldownMs` | `1500` | Minimum time between two events from this detector. |
| `minConsecutive` | `2` | Matching samples in a row needed to fire. |
| `sampleFps` | `6` | Samples per second. The indicator must stay on screen for at least `minConsecutive / sampleFps` seconds. |
| `scales` | `[]` | Extra template scales to try; `1.0` is always tried. |
| `method` | `"ncc"` | `"ncc"` compares brightness. `"edges"` compares outlines (edge strength), so an icon drawn over a changing scene (snow one round, a dark corridor the next) still matches. Prefer `edges` for HUD icons over gameplay. |
| `rearmDrop` | `0` | For indicators that stay up and pulse once per event (e.g. a kill emblem that flashes again on each kill of a multikill): fire again when the score dips by at least this much and recovers, still respecting `cooldownMs`. Try `0.1`. `0` = off. |

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

## Event types and icons

Every event has a type: an id (the `event` a detector emits), a name, one of 15 icons and a colour.
Built-in types: kill, death, assist, multikill, ace, objective, round start, round won, round lost, MVP,
bomb planted, bomb defused, bookmark, match start and match end.

You can add your own per game in Detector Studio ("+ New event type": any name, pick an icon). They are
stored in the game's `game.json` under `eventTypes` and travel with the game folder when you share it:

```json
"eventTypes": [
  { "id": "ult_used", "label": "Ult used", "icon": "zap", "color": "#f2c46d" }
]
```

Icons: `crosshair skull handshake flame crown gem flag trophy circle-x star bomb shield bookmark zap heart`.

## Rounds

`round_start`, `round_win` and `round_lost` (ids `round_start`, `round_win`, `round_loss`) are round markers.
The timeline draws a divider per round (R1, R2…) and the Library shows the round score. CS2 reports rounds
through Game State Integration; Valorant, Marvel Rivals and R6 Siege ship round detectors you finish in Studio
by capturing the round banners.

## Matches: how Recap knows a match started

Recap starts recording when a game's process starts and stops when it exits, so a recording is a whole game
session (menus included). Match boundaries come from:

- **API games**: League (the in-game process *is* the match) and CS2 (GSI reports match start and game over).
- **Detectors**: make a `match_start` detector (e.g. the map loading screen) and/or a `match_end` detector
  (the VICTORY/DEFEAT screen; put both screens on one detector as two reference images).

With *Split sessions into matches* on (Settings → Detection, default on), each detected match becomes its own
Library entry and video file (lossless cut, 5 s before the start and 8 s after the end kept), menu time is
dropped, and the session recording is removed. With only `match_end` detectors, each match runs from the
previous end; with only `match_start`, until the next start. Segments shorter than a minute are ignored.

## Several references: ignoring the scene behind an element

HUD elements are often drawn over the game: the scene shows through around and inside them,
so a single reference also "remembers" whatever was behind it that time. Give a detector two or
more references of the **same element cut with the same box** over different backgrounds (in
Studio: open the detector, scrub to another appearance, **Add this frame as a reference**).
Recap lines the references up, keeps only the pixels they agree on, and compares just those.

On real Valorant footage this lifted kill-emblem matches on unseen clips from about 0.57 to 0.98
while non-kill frames stayed below 0.3. References that show different things (say, two
different agent icons) are not masked against each other; each still matches on its own.

## Match details: screen readers

Besides events, a game folder can describe how to read **match details** (character, map, mode,
K/D/A, result, score) from the screen. Readers live in `readers/<id>.json`; Detector Studio's
**Details** mode creates them by drawing boxes, like detectors.

```json
{
  "id": "end-kda",
  "label": "Your K/D/A",
  "field": "kda",
  "kind": "text",
  "region": { "x": 0.104, "y": 0.563, "w": 0.145, "h": 0.054 },
  "slots": { "count": 5, "stepX": 0.162 },
  "highlight": { "box": { "x": 0.1, "y": -1.2, "w": 0.8, "h": 0.95 }, "color": "#e2e88f", "tolerance": 40 },
  "when": "end",
  "pattern": "(?P<kills>\\d+)\\D{1,4}(?P<deaths>\\d+)\\D{1,4}(?P<assists>\\d+)"
}
```

| Key | Meaning |
|---|---|
| `field` | Detail to fill: `character`, `map`, `mode`, `kills`, `deaths`, `assists`, `result`, `score`, or any custom name. Named groups in `pattern` fill those fields instead. `team_score` + `enemy_score` become `score` and `result`. |
| `kind` | `text`: text recognition with the OCR engine built into Windows (offline). `choice`: the labelled reference image that matches best (agent portraits, VICTORY/DEFEAT banners), using the detector matcher. |
| `region` | The box to read, as fractions of the game picture. |
| `slots` | Repeated boxes, e.g. one card per player: slot *i* is the region moved by *i* × `stepX`/`stepY`. |
| `highlight` | Read only the slot showing this colour (your own row or card is usually highlighted). `box` is relative to the slot and may reach outside it (e.g. the name above the K/D/A). |
| `when` | `end` (default): from 5 s before the match-end marker to the next match. `start`: the first minute. `any`: the whole match. |
| `sampleFps` | Frames read per second (default 1). Every frame votes; the most common reading wins. |
| `pattern` | Text only: a regular expression the reading must match. Letters that OCR confuses with digits are fixed in number-like tokens. |
| `values` | Text only: allowed values; readings snap to the closest (one typo per four letters) or are dropped. Use it for maps, agents and modes so a misplaced box yields nothing rather than nonsense. |
| `options` | Choice only: `[{ "value": "Jett", "templates": ["templates/jett.png"] }]`. |
| `threshold`, `method`, `templateFrameHeight` | Choice only: as for detectors. |

Where details come from, best first: what you typed on the match page, a local game API
(League Live Client, CS2 GSI, the Riot Client on 127.0.0.1 for VALORANT map/mode/score), screen
readers, and finally counts of detected events. Your edits are never overwritten by a rescan.

The bundled VALORANT readers are positioned from Riot's published end-of-match screen; if yours
differs, redraw the boxes in Studio.

## Recording each match

Recap records one entry per match, without relaunching the game:

* **VALORANT** and **CS2** report when a match is on (the Riot Client's local presence; CS2 Game
  State Integration). Recording starts when a match begins and stops a few seconds after it ends.
* **League of Legends / TFT**: the game process only exists during a game.
* **Other games** (Marvel Rivals, Rainbow Six Siege, custom games): Recap records while the game
  runs and cuts the session into matches with the game's `match_start` / `match_end` detectors,
  dropping menus. Set those detectors up in Detector Studio (the victory/defeat screen is a good
  `match_end`); without them a session stays one recording.

Pressing **Stop** skips the rest of the current match only; the next match records again.
