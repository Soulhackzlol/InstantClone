# Changelog

All notable changes will land here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versions follow
[SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

## [0.1.2] - Stop-start session restart fix

**The delay bar would freeze at 0% after Stop Streaming + Start
Streaming in OBS.** Reported by a streamer testing v0.1.1 against
their Enhanced Broadcasting setup ("stream no EB, stop, turn on EB,
try to apply delay - bar doesn't fill"). Not actually EB-specific:
any stop-start cycle reproduces it.

Root cause: OBS's RTMP wire timestamps restart from ~0 on every
fresh stream session. `begin_publish` correctly cleared the
per-track seq-header caches and the cached onMetaData, but left
the ring buffer's indexed tags from the prior session in place.
The old tags sat at the front of the index with high ts_ms values
(e.g. 600,000 ms into a 10-minute prior stream); the new session's
tags landed at the back with low ts_ms values (e.g. 100 ms).
`buffer_fill_ms = latest.saturating_sub(oldest)` saturated to 0
forever, and `trim_older_than` could not rescue it - its cutoff
also saturated to 0 against the new session's small current_ts.

Fix: `Ring::clear()` wipes the index (and the IDR-only secondary
index) on every fresh `begin_publish`. The seq counter and disk
write cursor are deliberately preserved so consumer seqs held by
destinations stay valid - new tags get seqs above the old high-water
mark and reads naturally advance onto fresh data. The on-disk bytes
get overwritten as new tags land. A regression test in
`buffer_fill_recovers_after_publisher_session_restart` reproduces
the exact streamer-reported symptom and fails fast (buffer_fill_ms
returns 0) without the fix.

**CI runners pinned to `windows-2022`.** GitHub is redirecting the
`windows-latest` alias to `windows-2025-vs2026` on June 15, 2026,
which would change the MSVC toolchain and runtime DLL set the
shipped binary links against - a silent upgrade that could regress
users on older Windows builds. Five lines across `release.yml`,
`ci.yml`, and `codeql.yml`. Bumping to a newer runner is now an
intentional choice rather than an upstream surprise.

## [0.1.1] - First-run dashboard polish

Quality-of-life fixes spotted right after the v0.1.0 release went out.

**Tour bubble for "Wire OBS to InstantClone" now matches the actual
OBS tab.** The step still described the manual Custom-RTMP path
exclusively even though the tab itself leads with the one-click
"Register with OBS" service-injection button. Rewrote the bubble
copy: recommended path first (click Register, restart OBS, pick
InstantClone from the Service dropdown), Custom RTMP framed as the
fallback for older OBS / multi-RTMP plugins / non-OBS encoders.

**OBS tab no longer shows a stale registration state.** If you
registered through the first-run wizard and *then* opened the OBS
tab, the tab still rendered "Not registered yet" with a primary
"Register with OBS" button - because the tab's status only refreshed
at page init and after its own button click, never on tab activation.
`showTab()` now re-queries `/obs/register-status` whenever the OBS
tab becomes active, so the button correctly flips to "Unregister
from OBS" the moment you switch to the tab.

## [0.1.0] - Enhanced Broadcasting + VOD audio mode

Headline: Enhanced Broadcasting works end-to-end through the proxy.
OBS Multi-track "Auto" lights up the Twitch transcoded ladder
regardless of account tier, and non-Twitch destinations keep getting
a clean single-resolution stream.

**Enhanced Broadcasting to Twitch.** New `POST /obs/multitrack-config`
endpoint proxies OBS's request to Twitch's `GetClientConfiguration`
API: we swap the `authentication` field, capture the session-allocated
IVS ingest URL + auth token, and rewrite the response's
`url_template`s so OBS uploads back through us. The supervisor stores
the IVS URL as a per-destination override and routes egress there
instead of the configured `live.twitch.tv` (only the IVS edge runs
the EB transcoder). Failure modes (Twitch API timeout, transport
error, non-2xx response) each fall back to a static config and log a
discriminated reason. Webhook + EB proxy now share a `native-tls`
connector helper (`src/https.rs`) - silently broken Discord webhooks
got fixed as a side-effect.

**Per-track sequence-header cache.** OBS sends one OneTrack-format
SPS/PPS tag per ladder rung. The old single-slot cache stomped them
all to the last-received one, which Twitch Inspector surfaced as
resolution "x" for tracks 1..N and the stream silently died at the
TCP retransmit boundary (~60 s). Cache is now keyed on the TrackId
byte (`h264::seq_header_track_id`); `send_sequence_headers` iterates
every cached track for Twitch passthrough and selects TrackId 0 for
non-Twitch destinations.

**Non-Twitch ladder-tag drop.** Per-frame OneTrack tags with
`TrackId != 0` are filtered out of the non-Twitch egress in
`select_video_bytes`. Before this fix, OBS's 4-rung ladder produced 5
single-track tags per PTS on YouTube - decoders read that as a
multi-frame storm, the connection dropped ~12 s after handshake, and
the supervisor reconnected on a loop. The primary still flows
through as a legacy AVC tag.

**One-click OBS service registration.** The wizard's primary path
adds an "InstantClone" entry to OBS's Service dropdown with the
`multitrack_video_configuration_url` pointing at our proxy. Idempotent
+ self-healing: re-registering with a changed `web_port` refreshes
the URL; a corrupted or write-locked `services.json` surfaces
specific errors ("close OBS Studio first" / "file may be corrupted").
A `.bak` is always written before patching.

**Wizard rewrite.** Two steps now: "Step 1 · Connect OBS to
InstantClone" (Register-with-OBS button vs. Custom-RTMP card with
copy-server-URL) then "Step 2 · Where should we forward your stream?"
(existing platform/key form). The OBS card polls
`/obs/register-status` so the button reflects current registration
state.

**Hardening.**

- `begin_publish` clears `video_seq_headers`, `audio_seq_header`, and
  the `onMetaData` cache on a fresh publisher session. Previously a
  publisher A→B reconnect could leak EB seq-headers into a non-EB
  session and freeze Twitch's decoder. Mid-session caches still
  survive an egress supervisor restart (regression-tested).
- `mark_ingest_dead` clears every destination's `eb_override_url` so
  a subsequent non-EB stream doesn't land on a stale IVS endpoint.
- The dashboard `⚡ Enhanced Broadcasting` chip lights when multi-track
  is observed; the Twitch mobile-decoder cap pill auto-suppresses
  while EB is active (the ladder bypasses the cap).
- Per-tag `TAG_VIDEO` trace formatting is now gated behind
  `trace::is_enabled()`, saving ~24 KB/s of allocation churn on idle
  builds.
- Fresh `DestinationState`s initialise `consumer_seq` to `u64::MAX`
  so a destination registered mid-stream doesn't briefly pin the
  ring's trim to seq 0.

**Multi-track audio + VOD-audio mode.** Mirror of the video EB fix
on the audio side: `classify_audio_tag` now detects nested seq-headers
in multi-track audio packets, `audio_seq_header` becomes a
per-TrackId BTreeMap, `send_sequence_headers` re-emits every cached
track for passthrough destinations, and `select_audio_bytes` drops
OneTrack tags with TrackId != 0 on non-Twitch egress. The whole
delay machine handles two-audio-track Enhanced-RTMP streams without
losing decoder config on cuts or reconnects. Phase A in commit
`bef752b`.

**Two-mode VOD-audio toggle (opt-in, off by default).** Per-Twitch-
destination toggle in the editor; two modes:

- **VOD audio mode** -on publisher-connect we ourselves call Twitch's
  `GetClientConfiguration` with `vod_track_audio: true`, get back a
  session-allocated IVS URL with a VOD-audio slot, stash it on
  `eb_override_url`. The streamer picks Custom RTMP in OBS; we
  flip `EnableCustomServerVodTrack=true` in OBS's `global.ini` so
  the VOD Track checkbox unlocks. Trade-off: no EB transcoded
  ladder on this path.
- **Also enable Enhanced Broadcasting (EXPERIMENTAL)** -sub-toggle
  inside VOD-audio. Injects
  `multitrack_video_configuration_url: http://127.0.0.1:<port>/obs/multitrack-config`
  into the active OBS profile's `service.json` (with a `.bak`). With
  the injection in place, OBS's Custom RTMP service auto-fetches our
  multitrack-video config -EB and VOD audio both fire on the same
  session. Only applies to the **active** OBS profile; switching
  profiles in OBS disables the injection until you re-toggle. We
  warn the user explicitly in the toggle's disclosure body.

`reconcile_obs_vod_files` runs after every destination upsert /
delete / config reset and keeps `global.ini` + `service.json` in
sync with what the dashboard says. `GET /obs/register-status` now
also reports `vod_audio_flag`, `vod_eb_injected`, and
`active_profile` for the UI status line.

**Tests: 88 → 132.** New coverage for the per-track audio
seq-header cache, multi-track audio classification, TrackId-drop
selection for non-Twitch audio, INI read/write round-trips, and
service.json inject/strip round-trips. Phase A regression test in
`begin_publish_purges_cached_sequence_headers_from_prior_sessions`
extended to seed two audio tracks plus the video four-track ladder.

## [0.1.0-beta.6] - audit follow-through + live delay adjustment + the Twitch source-only mystery solved

Headline: a wire-spec audit closed the last meaningful protocol gap,
the dashboard learned to adjust delay on-the-fly, and a full afternoon
of alt-account testing finally disentangled why some streams went
Source-Only at high bitrate - turns out it was never InstantClone.

**RTMP Acknowledgement (BYTES_READ_REPORT) on both directions.** Adobe
spec §5.4.3 and librtmp's actual code both require a peer to send msg
type 3 every time bytes received cross the window-ack-size/10
threshold. We accepted incoming acks silently but never sent any.
The `ChunkReader` now counts every wire byte (chunk headers + extended
timestamps + payload), captures the peer's Window Ack Size from msg
type 5 (defaults to 2.5 MB until the peer overrides), and exposes
`take_pending_ack()` so the ingest server and egress reader-drain can
emit `BYTES_READ_REPORT` inline. At 6 Mbps that's about one 4-byte
message every 0.4 s - invisible perf cost, but lets strict RTMP
relays accept InstantClone as a well-behaved peer when it's used as
itself a re-publish source.

**AMF0 Strict Array decode.** The connect-properties bag we ship for
Twitch OBS-parity includes an Enhanced-RTMP `fourCcList`, which is
encoded as Strict Array (marker `0x0A`). Real RTMP servers handle
that marker fine - the published beta.5 binary streams correctly to
Twitch, YouTube, Kick - but the in-tree sink used for e2e CI shares
the same decoder, which only knew `0x00`-`0x08`. So the sink rejected
our own connect and the e2e job went red. Adds a
`StrictArray(Vec<Amf0>)` variant with a depth-bounded recursive
decoder; the public binary is unchanged.

**Enhanced-RTMP `PacketTypeMetadata` (=4) classified distinctly.** The
video classifier was implicitly treating PacketType=4 (mid-stream
HDR `colorInfo` updates) as an arbitrary non-seqheader packet. Adding
`is_metadata: bool` to `VideoTagInfo` lets the trace log distinguish
these from anomalous packets, and tightens `is_keyframe` to refuse
IDR-cut treatment on PacketType ∈ {0, 4} (neither carries slice data;
flagging them as IDR would invent a bogus cut point if the encoder
set FrameType=1 anyway).

**Capacity-aware UI, no more silent buffer-cap stalls.** The default
`buffer_mb` bumps 300 → 500. The System tab gains a live "X MB → max
Ys delay at 10 Mbps reference (currently ≈ Zs at N.N Mbps in)"
hint that updates as the user types. The cockpit's delay input and
profile chips refuse to arm a delay larger than the buffer can hold
at the current (or planning-default) bitrate, with an explicit "needs
≥ N MB" reason instead of the previous silent stall. Math accounts
for ~160 kbps audio, ~5 % RTMP framing overhead, and a 3 % safety
headroom.

**Live delay adjustment (re-arm + adjust-up + adjust-down).** The
controller had `arm_delay()` wired for live changes since day one,
but the cockpit hid it: the delay input was logically dropped in
armed / active states and the CTA only offered Activate / Cut. The
cockpit now treats a typed value that differs from the current
`armed_delay_ms` as the new target - replacing Activate with
"↻ Re-arm at Ns" while armed, or Cut delay with "↻ Adjust ↑/↓ to
Ns" while active. Profile chips and rows stay clickable in both
states with adaptive tooltips. A transient "Adjusting → rewinding to
Ns (currently X.Xs)…" sub-text shows during the brief window where
the controller is seeking to a larger delay than what's currently
being delivered.

**Twitch source-only / mobile-decoder warning.** A full afternoon of
testing on a no-Affiliate alt account established two things that
beta.4 had bundled into one bug:

- The transcoded quality ladder is *account-tier* gated, not
  bitrate-gated. Non-Affiliate accounts get "Transmuxed (Source-Only)"
  at any bitrate; Affiliate / Partner get the ladder.
- The viewer-side "Error #1000 / black screen with audio" symptom is
  a mobile-decoder ceiling, not a Twitch transcode issue. At ≤ 8 Mbps
  1080p60 H.264 Source-Only most mobile hardware decoders cope; above
  ~8 Mbps they start failing. Verified empirically on the alt - 8k
  Source-Only plays clean on phone and PC, 10k Source-Only reproduces
  the beta.4 viewer breakage.

A new header chip (`⚠ Twitch · mobile risk at N.N Mbps`) lights when
any Twitch destination is alive at > 8 Mbps and a click toast lays
out the tier-vs-bitrate distinction so users blame the right layer.
The Twitch entry in `/platforms` tip gets rewritten with the same
framing for destination-creation-time discovery. OBS's built-in
Twitch *preset* auto-caps via Twitch's API; OBS's "Custom server"
path skips that check, and InstantClone is necessarily a
Custom-server target - so the cap-skip behaviour is structural, not
a wire bug. The warning is the right intervention.

**Overlay number stops jittering in active phase.** The on-stream
overlay's seconds readout used to wobble 15.8 → 15.9 → 16.0 because
it was wired to `current_delay_ms`, which is recomputed each pump
tick from the slowest consumer's frame timestamp and doesn't land on
round boundaries. Switched to `target_delay_ms` (== armed in active
phase) - stable to the tenth-of-a-second the streamer actually
intended. Dashboard hero already did this.

**ChunkReader internal refactor.** The Acknowledgement work
introduced a temporary 9-element Option tuple for threading parsed
header fields between the read phase (holds &mut self) and the
state update phase (holds &mut streams). Replaced with a private
`ChunkHeader` enum returned by a `read_chunk_header` helper -
~130 fewer lines, no unused-binding suppressions, same wire
behaviour. Also dropped the speculative `tray.rs::update_tooltip`
shim that was carrying a `#[allow(dead_code)]` for hypothetical
future use.

### Known issues

- Long-session behaviour (multi-hour streams) unproven. Longest
  validated run is ~20 min.
- YouTube-via-restream worked in prior testing but hasn't been
  re-validated post-OBS-parity. Wire layer is unchanged so no reason
  to expect regression, but it's listed here for honesty.
- Twitch bandwidth-test mode does not route through the full
  transcoder, so the actual transcoder-lane decision can only be
  confirmed by a real (non-`?bandwidthtest=true`) stream - which the
  alt-account work in this release does cover.

## [0.1.0-beta.5] - full OBS parity on the wire + per-platform onboarding

Headline: InstantClone now looks indistinguishable from OBS to every
RTMP ingest it talks to, and the dashboard tells you where to find
your stream key and which platforms have hidden gotchas.

**Full OBS parity in the publish handshake.** The `connect` command
now carries the same property bag OBS does - `audioCodecs=3191`,
`videoCodecs=252`, `videoFunction=1`, `objectEncoding=0`,
`capabilities=239`, `fpad=false` - plus the Enhanced-RTMP
`fourCcList` (`avc1, hvc1, av01, vp09, mp4a, Opus, ac-3, ec-3, fLaC`)
so transcoder lanes know we can pass through HEVC / AV1 / Opus. This
is the real fix for the beta.4 known issue: Twitch was downgrading
streams above ~6 Mbps to Source-only because we hadn't declared codec
support, so the transcoder didn't pick the stream up. Other RTMP-layer
changes match OBS bit-for-bit: chunk size negotiated to 60 000,
1 MB `SO_SNDBUF` for big-bitrate headroom, and `FCUnpublish` sent
before `deleteStream` on graceful close.

**RTMP Ping Request / Response on both sides.** Some RTMP servers (OBS
itself when it's our peer; certain CDN edges) ping us periodically to
verify we're still consuming. We now parse User Control Message event
type 6 (Ping Request) on both the ingest and egress reader paths and
echo back event type 7 (Ping Response) with the original timestamp on
CSID 2. Without this, idle keepalives eventually fired but RTMP-layer
protocol expectations weren't being met.

**15-second publish timeout, end of the 60-second startup stall.**
`await_command_status` now bails after 15 s of no `onStatus` instead
of letting the dead-server case ride the TCP retransmit clock. First
publish after a network blip used to stall for ~60 s; the supervisor
now reconnects fast enough that the user sees one missed frame instead
of a minute of dead air.

**Wire-level trace becomes a UI toggle.** The advanced trace
(`./instantclone-trace.log`) used to require the `INSTANTCLONE_NO_TRACE`
env var to disable. There's now a checkbox in the System tab that
flips an atomic at runtime - no restart, persists to settings. Default
on for the beta so traces are shippable without flipping a flag.

**Per-platform stream-key help.** The wizard and destination form now
render a help block under the key input with a deep link to the
platform's dashboard page ("Twitch Creator Dashboard → Settings →
Stream → Primary Stream Key") and a one-line tip. Updates live when
you change the platform dropdown. The Kick tip is styled as a
warning: Kick runs on AWS IVS which silently drops streams with
B-frames enabled, and most OBS encoders default B-frames on. The
recommended-settings panel in the OBS tab now spells out exactly
which encoder switches to flip.

**Chat in OBS (browser dock builder).** New section in the OBS tab:
pick Twitch / YouTube / Kick / Trovo, paste your channel name (or, for
YouTube, the live broadcast video ID / watch URL), copy the URL,
add as an OBS Custom Browser Dock. No more leaving the app to find
the popout-chat URL pattern.

**Destinations tab graphics auto-update.** Saved-destination cards
were only updating their bitrate sparkline / alive pill on explicit
user actions. `applyState` now merges the live per-destination fields
from `/state` into the rendered list on every tick.

### Known issues

- None new since beta.4. The Twitch ≤ 6 Mbps workaround documented
  in beta.4 should no longer trigger for most users now that the
  codec hints are declared, but is left in place as a fallback in
  case any transcoder lane still misbehaves on a given stream.

## [0.1.0-beta.4] - overlays redesigned, dashboard quirks fixed, Twitch caveat documented

Headline: the overlay system gets a full pass and two real dashboard bugs
get killed.

**Overlays.** The 9-style grab-bag becomes a curated 6 with a shared design
language: minimal, corner, strip, focus, broadcast, ticker. Three
behaviours apply across all of them - overlay auto-dims to ~22% after 4 s
of idle/passthrough, the big delay number tweens between values instead
of snapping, and a brief accent halo blooms on every phase transition so
the moment of arm/activate/cut is felt rather than guessed at. The
`ticker` style actually scrolls now (it was a static bar pretending). The
`overlays/` folder is cleaned of the three stale standalone duplicates
(`minimal.html`, `corner.html`, `strip.html`) and gets a single
well-commented `custom-template.html` that documents the `/state` JSON
contract so you can fork your own.

**Destinations tab finally auto-updates.** The cards (bitrate sparkline,
alive pill, status text) were only refreshing on explicit user actions -
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
outcome - empty URL, connection error, non-2xx HTTP from Discord (with
status), or timeout.

**Platform polish from beta.3 testing:**

- `flashVer` now reports `"FMLE/3.0 (compatible; FMSc/1.0)"` to match OBS
  exactly - some platforms gate transcode behaviour by this string.
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
  early-beta users may have bookmarked are gone - use
  `/overlay?style=minimal` instead (same renderer, but newer, with the
  unified design language and behaviours described above).

## [0.1.0-beta.3] - first-run UX pass

The onboarding tour now actually fires on first launch: beta.2 had it
implemented but gated behind a check that the wizard silently sidestepped,
so first-run users never saw it. Moved the trigger into the
wizard-to-dashboard transition where it belongs.

Wizard now has a subtle "Not now - let me look around first" link for
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

CI: real end-to-end job - ffmpeg pushes a synthetic H.264 + AAC stream
into the proxy, sink confirms publish + IDR + audio frames on every push.
CodeQL workflow added (skipped while the repo is private; auto-runs once
it goes public).

Still not tested against real Twitch / YouTube / Kick ingests - same gap
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
