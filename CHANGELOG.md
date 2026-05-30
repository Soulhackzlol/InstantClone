# Changelog

All notable changes will land here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versions follow
[SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet. First public push lives below.

## [0.1.0-beta.4] - overlays redesigned, dashboard quirks fixed, Twitch caveat documented

Headline: the overlay system gets a full pass and two real dashboard bugs
get killed.

**Overlays.** The 9-style grab-bag becomes a curated 6 with a shared design
language: minimal, corner, strip, focus, broadcast, ticker. Three
behaviours apply across all of them — overlay auto-dims to ~22% after 4 s
of idle/passthrough, the big delay number tweens between values instead
of snapping, and a brief accent halo blooms on every phase transition so
the moment of arm/activate/cut is felt rather than guessed at. The
`ticker` style actually scrolls now (it was a static bar pretending). The
`overlays/` folder is cleaned of the three stale standalone duplicates
(`minimal.html`, `corner.html`, `strip.html`) and gets a single
well-commented `custom-template.html` that documents the `/state` JSON
contract so you can fork your own.

**Destinations tab finally auto-updates.** The cards (bitrate sparkline,
alive pill, status text) were only refreshing on explicit user actions —
saving a form, switching tabs, or F5. The `/state` poll already carried
the live per-destination fields; `applyState` now merges them into the
cached list on every tick so the cards stay live.

**Discord webhook test endpoint stops lying.** It used to call the same
fire-and-forget path as the destination/ingest notifications, which
silently dropped empty-URL cases, was suppressed by the 2-second throttle
right after a destination-connect notification, and swallowed any HTTP or
TLS error from Discord. Result: "test fired" toast, nothing reaches
Discord, no way to diagnose. Replaced with an explicit synchronous path
that validates the URL, bypasses the throttle, and surfaces the real
outcome — empty URL, connection error, non-2xx HTTP from Discord (with
status), or timeout.

**Platform polish from beta.3 testing:**

- `flashVer` now reports `"FMLE/3.0 (compatible; FMSc/1.0)"` to match OBS
  exactly — some platforms gate transcode behaviour by this string.
- Wire-level egress trace to `./instantclone-trace.log` (opt-out via
  `INSTANTCLONE_NO_TRACE=1`) records every handshake event, AMF0
  command, sequence header, cut, and tag for offline diagnosis when
  something looks weird platform-side.
- Sequence headers are re-emitted on every cut now. The previous "Twitch
  caches them, don't re-emit" assumption was optimistic; some transcoder
  workers don't share that cache.
- Cut log format actually shows the new `output_ts_base` and
  `seq_header_gen` instead of `OLD→OLD+1`.
- Egress destinations close cleanly (`deleteStream`) when OBS goes away,
  and the supervisor refuses to respawn while ingest is dead so we don't
  burn TCP cycles to Twitch / YouTube while there's nothing to send.
- YouTube backup ingest URL now correctly appends `?backup=1` so
  YouTube's edge enables real fail-over instead of treating it as a
  duplicate primary.

### Known issues

- **Twitch streams above ~6000 Kbps lose the transcoded ladder.** Twitch
  produces transcoded qualities (720p60 / 480p / 360p / 160p) only for
  streams within its documented bitrate ceiling. Above that, viewers
  receive Source-only quality, which means PC viewers on slower
  connections hit `Error #1000` and mobile viewers see a black screen
  with audio still playing (the mobile hardware decoder can't handle the
  Source resolution / bitrate combo). InstantClone is a transparent
  pass-through and cannot re-encode without bundling ffmpeg or NVENC
  (dozens of MB, platform-specific GPU dependencies, defeats the
  "tiny binary" point). Workaround: set OBS to ≤ 6000 Kbps for guaranteed
  transcoded ladder. Twitch Partners can sometimes push higher with
  Auto-Transcode access. A tier-aware in-app warning chip is planned for
  a follow-up release.
- The on-disk overlay file paths (`/overlay/minimal.html` etc) that some
  early-beta users may have bookmarked are gone — use
  `/overlay?style=minimal` instead (same renderer, but newer, with the
  unified design language and behaviours described above).

## [0.1.0-beta.3] - first-run UX pass

The onboarding tour now actually fires on first launch: beta.2 had it
implemented but gated behind a check that the wizard silently sidestepped,
so first-run users never saw it. Moved the trigger into the
wizard-to-dashboard transition where it belongs.

Wizard now has a subtle "Not now — let me look around first" link for
people who want to poke the dashboard before committing to a destination.
`configured=false` stays on disk so the wizard returns next launch.

Tour copy is more honest about pre-stream state: OFFLINE is the first
listed delay-readout state with "(where you probably are right now)", and
the OBS step explicitly says the dashboard will read "not connected"
until OBS hits Start Streaming. Welcome card lands smoothly in the centre
with a 450ms spring entrance (beta.2 anchored it at the top-left of
50%/50% because of a transform conflict).

`instantclone.exe sink` from PowerShell prints its banner + live stats
again. Was silent in beta.2 because the release binary builds as
`windows_subsystem = "windows"` and the sink CLI's `println!`s wrote
into the void. Now attaches to the parent console (or allocates a fresh
one for double-click invocations) before dispatching.

CI: real end-to-end job — ffmpeg pushes a synthetic H.264 + AAC stream
into the proxy, sink confirms publish + IDR + audio frames on every push.
CodeQL workflow added (skipped while the repo is private; auto-runs once
it goes public).

Still not tested against real Twitch / YouTube / Kick ingests — same gap
as beta.1 / beta.2 between here and `v0.1.0`.

## [0.1.0-beta.2] - tag-on-fmt-clean rebuild

Same code as beta.1 from the user's point of view. beta.1 was tagged on
a pre-`cargo fmt` commit, so the strict CI gate rejected it and no
release artefact published. beta.2 is the same content built from the
fmt-clean commit. Use this if you grabbed nothing from the beta.1 release
page.

## [0.1.0-beta.1] - first public pre-release

End-to-end tested locally against the in-tree RTMP sink with `ffmpeg`
publishing a synthetic H.264 + AAC source: ingest, codec classification,
forwarding to destination, and the full `arm → activate → cut → disarm`
state machine all behave correctly.

**Not yet tested against real Twitch / YouTube / Kick ingests.** That's the
gap between `-beta.1` and the eventual `v0.1.0`. If you're a streamer
willing to try it on a low-stakes session and report back, the bug
template is in [Issues](https://github.com/Soulhackzlol/InstantClone/issues).

See `0.1.0` below for the full feature list.

## [0.1.0] - first public release

### Added

- Two-phase delay state machine: arm → preparing → ready → active → cut.
  Activation is instant on screen; cuts are IDR-aligned with monotonic
  timestamp rewriting so destination players don't glitch.
- Multi-destination egress. One OBS feed fans out to Twitch + YouTube +
  Kick + custom RTMP endpoints, each with its own enable toggle and per-
  destination bitrate / reconnects / cut counters.
- Disk-backed ring buffer (`./instantclone.buf`, default 300 MB ≈ 7 min
  at 6 Mbps). In-memory IDR index with O(log n) seek. Reset on every
  clean shutdown.
- Hand-rolled HTTP/1.1 web UI: full dashboard at `/`, compact OBS dock at
  `/dock`, themable browser-source overlays at `/overlay?style=…` (9 styles,
  5 languages: en, es, pt, fr, de).
- Server-Sent Events stream at `/events` for live state push (replaces
  polling). Polling fallback for clients that don't support SSE.
- Pre-gzipped HTML payload at build time (~115 KB source → ~25 KB gzipped).
  Build pipeline minifies + gzips via `flate2` as a build-only dep.
- HTTP API for Stream Deck / scripts: `POST /arm`, `/activate`, `/disarm`,
  `/stop`, `/delay`. CSRF guard on POSTs via Origin/Host comparison.
- Tray icon (Windows) with status header, conditional **Cut delay**,
  **Open dashboard / dock**, **Copy RTMP URL**, **Quit**. Hand-rolled `.ico`
  generated at build time. Runs with no console window
  (`#![windows_subsystem = "windows"]`).
- Port-conflict pre-flight: detects the owning PID + executable name via
  `GetExtendedTcpTable` + `QueryFullProcessImageNameW`, pops a native
  modal asking the user to switch to the next free port or quit, persists
  the choice.
- 86 unit tests covering the state machine, AMF0 codec + recursion guard,
  H.264 + Enhanced RTMP IDR detection, ring-buffer eviction with in-flight-
  read protection, port-conflict FFI roundtrip, HTTP parsing, CSRF policy,
  config form parsing + save/load round-trip, and `accepts_gzip` content
  negotiation.

### Notes

- Windows-only at this version. The code is mostly portable but the tray,
  port pre-flight, and process / RSS sampler have Windows-specific paths.
- No automated release pipeline yet. Build from source per the README.

[Unreleased]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Soulhackzlol/InstantClone/releases/tag/v0.1.0
