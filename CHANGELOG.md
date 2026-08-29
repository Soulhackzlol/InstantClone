# Changelog

All notable changes will land here. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versions follow
[SemVer](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.14] - Hotkeys and MIDI for the delay, plus IPv6 ingest

### Drive the delay without alt-tabbing

Arming a delay meant leaving the game to find the dashboard, which is the
one moment you cannot afford to be looking somewhere else. Five actions are
now bindable to a global hotkey, a MIDI pad, or both (thanks **@4amMagic**
on X for the idea!).

- **Five bindable actions.** Delay on/off, arm or disarm the buffer,
  activate the armed delay, cut back to live, and schedule a cut for the
  moment the current live edge airs. The arm key toggles: press it again to
  free a buffer you armed by mistake. It refuses to disarm while a delay is
  on air, since that would drop every viewer to live - **Cut to live** is
  the key for that. All configured in **Settings**, and re-registered
  the instant you save through the tray's message loop, so nothing needs a
  restart.
- **Hotkeys fire under a fullscreen game.** Registered with Win32
  `RegisterHotKey`, so a binding lands while OBS or the game holds focus.
  Every binding must carry at least one modifier: a bare keypress mid-match
  can never trip a delay action by accident, and that requirement is the
  whole misclick guard.
- **Modifier-plus-key only, deliberately.** Full chords would need a
  low-level keyboard hook, which is the same API a keylogger uses and gets
  an unsigned binary flagged by AV vendors. Not a trade worth making for
  five actions.
- **MIDI with a learn mode.** A background listener (winmm) watches every
  MIDI input device, so a deck or pad is bound by pressing it rather than by
  hunting for a note number. Note-on with velocity 0 is a note-off in
  disguise and is ignored, and a control change only counts as a press at
  value 64 or above, so releasing a button never double-fires. Both paths
  route through the same action handler as the keyboard, so a pad and a
  hotkey behave identically. Marked experimental for this release: it works,
  but it has had far less real-world use than the rest of the app.
- **One control, one action.** Binding a combo or a pad that another action
  already holds moves it, rather than leaving a second entry that the
  dashboard shows as bound and that could never fire: Windows refuses the
  second registration of a combo, and a MIDI signature matches the first
  action that claims it. The MIDI toast names the action it was moved off.
- **A combo another app owns says so on the row.** Windows refuses to
  register a hotkey somebody else holds (try Win+L), and the binding then
  sits in Settings looking set while nothing ever fires. The row now flags
  it and the field turns amber, updated live, so the fix is one keypress
  away instead of a hunt through the log.
- **Recording a combo no longer fires it.** A registered hotkey is handed to
  us by Windows instead of to the focused window, so pressing an already
  bound combo in the capture field ran its action and the field never saw
  the key - rebinding one was impossible. Global hotkeys now stand down
  while the dashboard is recording, and come back when it commits, cancels,
  leaves the tab or closes the page. The backend holds a 30 second deadline
  rather than a flag, so a dashboard that dies mid-capture costs one window,
  never a session without hotkeys.
- **A combo that other apps use warns you.** A global hotkey is taken from
  every app on the machine, so binding Ctrl+V means paste quietly stops
  working everywhere until InstantClone quits. Recording one of those now
  says so, and still binds it - it is your machine.
- **You can see which binding fired.** Press a hotkey or a pad and the
  dashboard flashes that row and toasts what ran, so a press that landed
  while you were looking at a game is still visible when you come back. A
  refusal flashes amber and carries the reason instead.
- **Two controllers, different bindings.** A mapping records which device
  it came from, so pad 1 on one deck and pad 1 on another are two different
  controls even though both send note 36 on channel 1 - which is what half
  the controllers ever made send. The row shows the device, and a mapping
  written by hand without one still matches any device.
- **Pick which MIDI device listens.** The listener took every input it could
  open, so a keyboard or a control surface sharing the desk could fire delay
  actions of its own. **Settings → MIDI controller → Listen to** narrows it
  to one device, and the line underneath says what is actually being heard -
  including when the device you picked is unplugged, which used to read the
  same as listening to it.
- **A refused action tells you why, where you are.** Press activate before
  the buffer is ready, or arm while a delay is on air, and a tray balloon
  says so. Only refusals raise one - anything that worked stays silent, so
  a balloon never lands on a display-captured scene during normal use.
- **A delay armed by pad or key survives a restart.** Those paths run
  outside the dashboard, which is what used to write the delay state to
  disk, so they now ask the runtime to persist on their behalf.

Windows only for now. The config fields parse and round-trip everywhere, so
a config shared with a Linux build stays valid, and the dashboard hides the
sections on a build with no backend rather than showing dead controls.

- **Updating repairs your OBS service entry by itself.** The entry
  InstantClone writes into OBS carries the addresses OBS uses to reach it, and
  0.1.13 wrote an IPv4 literal there. That still works unless OBS's **IP
  Family** is set to IPv6, where it cannot resolve at all - and until now
  fixing it meant noticing the **Register** button had quietly changed back
  and pressing it. If you had registered before, it is repaired for you.
  Updating with OBS open is fine: OBS keeps that file in memory and writes it
  back when it closes, so the repair waits for that and lands before the next
  time OBS reads it. Your current session is unaffected either way, and an
  entry you never made, or deliberately removed, is left alone.

### Lock the dashboard down, and run it on Linux

Two things this release ships that are easy to miss, because neither is on
the delay path.

- **Optional dashboard password.** Off by default and nothing changes if you
  leave it that way. Turn it on in **System → Network** when the dashboard is
  reachable from anywhere but your own machine. Hashed with PBKDF2-SHA256 and
  a per-install salt, with rate limiting and a lockout after repeated failed
  attempts; the first password can only be set from the local machine.
  **If you forget it,** delete the `dashboard_password_hash=` line from
  `instantclone.config.json` next to the exe and restart. There is no reset
  button, by design: a reset button reachable from the network is not a lock.
- **Optional ingest key.** Also in **System → Network**. Requires OBS to send
  a matching stream key before a publish is accepted, which is what stops a
  second machine on your LAN from pushing into your delay when you have
  turned on "listen on all interfaces".
- **OBS dock token.** The dock gets its own least-privilege credential rather
  than your password, so a docked panel inside OBS cannot change settings.
- **Linux (x86-64) builds are published.** `instantclone-v0.1.14-linux-x64`
  runs headless on a VPS or on an Ubuntu desktop. There is no system tray
  there, so the dashboard is the whole control surface and Quit and Restart
  live in its System tab. Global hotkeys and MIDI stay Windows-only, since
  both sit on Win32 APIs. For anything network-facing, pair the dashboard
  password with TLS through a reverse proxy.

### Fixes

- **Nothing to delay now means nothing happens, and it says so.** With no
  encoder connected the buffer cannot fill, so arming dropped you into
  "preparing" forever and activating was a countdown that never moved. Every
  surface - hotkey, MIDI pad, dashboard, dock - now refuses to build a delay
  while nothing is publishing, with one message that names what it is
  waiting for. Anything that *removes* delay still works, because OBS
  crashing mid-delay is exactly when someone needs to cut back to live.
  Auto-arm on connect is unaffected: it fires on the connect itself, when a
  publisher is already there.
- **"Start with Windows" died at launch with "Access is denied (os error
  5)".** Turning the setting on and logging back in popped the buffer-file
  error before InstantClone ever came up (thanks **@GM0NIE** on X for the
  report). Windows launches startup entries from `C:\Windows\System32`, and
  every default path we use is relative to wherever we were started from -
  so the config sitting next to the exe was never read (every setting
  silently back to its default) and the delay buffer was aimed at a folder
  no unelevated program may write. InstantClone now anchors itself to the
  folder holding the exe before it touches a single file, so autostart, a
  shortcut and a plain double-click all keep the same files in the same
  place. The buffer error dialog also reports the full resolved path now
  instead of `./instantclone.buf`, so a genuinely unwritable folder names
  itself. Set `CONFIG_PATH` to keep your own layout.
- **The clear button on a MIDI mapping did nothing.** It reported success,
  emptied the row, and left the mapping in place: the pad kept firing its
  action and the row came back on the next dashboard load. The config route
  simply did not accept `midi.<action>` keys, only `hotkey.<action>` ones,
  so there was no way to unmap a control at all. Both now go through one
  allow-list with a test that fails if the two ever drift again.
- **A learn request nobody finished could bind a control days later.**
  Learn mode swallows the next press instead of running it, and closing the
  tab left it armed forever: a pad press mid-stream vanished, and the next
  time the dashboard opened it was committed as a binding. Learning now
  stops when you leave the System tab, hide the tab or close the page, and
  the listener expires it after 30 seconds regardless, for the browser that
  is killed outright and never gets to say so.
- **Hotkeys were torn down and rebuilt on every arm, activate and cut.**
  Each of those writes the delay state, and any settings write re-registered
  the whole set: a press landing in that window was lost, and a combo owned
  by another app repeated its warning in the log all session. It now
  re-registers only when a binding actually changed.
- **A MIDI controller swapped between polls went unnoticed**, because the
  listener only reconciled when the device *count* changed. It compares the
  device names now, and retries a controller that another app held open at
  startup instead of waiting for the count to move.
- **The IPv6 ingest listener could retry a port forever.** If something else
  owned `[::1]:1935` for good, that leg retried once a second for the rest
  of the session with nothing said in the dashboard. It now retries for 30
  seconds (long enough for a restart to hand the port over), then logs one
  line and stops. IPv4 is where all but a handful of setups connect and it
  is unaffected either way: that leg still retries forever, because giving
  up there would mean no ingest at all.
- **The Register button could stick on a services.json OBS had rewritten.**
  We found our own entry by walking backwards from its name to the nearest
  `{`, which lands inside a string when any key ordered before `name` holds
  one - a `url_template` carrying `{stream_key}` is exactly that shape. The
  span then covered something that was not our entry, so registration always
  read as stale. It is a single string-aware pass now, which returns the
  object that actually encloses our name whatever the key order.
- **"Failed to connect" when OBS is set to IP Family = IPv6.** OBS reported
  `Error reaching host. Make sure that the interface you have bound can
  access the internet...`, which points at a firewall and is the wrong place
  to look (thanks **@GM0NIE** on X for the report and for testing the fix). 
  **Settings -> Advanced -> Network -> IP Family** is handed straight
  to `getaddrinfo` as the address family, and `127.0.0.1` does not resolve
  under IPv6 - the lookup fails with `WSAHOST_NOT_FOUND`, and librtmp
  relabels it as an unreachable host because a bind IP is set. Two things
  were wrong on our side: the address we hand OBS was an IPv4 literal, and
  we only ever listened on IPv4, so a resolvable name would have found
  nothing waiting.

  The ingest now listens on both families, and the server we register is
  `rtmp://localhost:1935/live`. `localhost` resolves under either setting,
  and OBS races every resolved address, so one entry covers both without
  asking you to pick. IPv6 is best effort: a machine with it disabled logs
  one line and carries on over IPv4 exactly as before. **Re-register the
  server in Settings after updating**, or pick the refreshed entry in OBS,
  since the old IPv4-literal entry stays until you do.
- **The Register button could stick on "registered" with no way back.** The
  check compared against the whole of `services.json`, so an entry named
  like ours living inside another service's server list pinned the button
  permanently. It now looks only at our own entry.
- **Registering could target the wrong OBS profile.** We located the active
  profile by its display name, but that is not the folder name: OBS
  sanitises the folder and records it separately. A profile whose name
  contains a character OBS strips resolved to a path that does not exist, so
  registration silently landed nowhere. It now reads the recorded directory,
  and falls back to matching profiles by name for older layouts

- **A delay running for 49.7 days straight would empty its own buffer.**
  RTMP timestamps a stream in unsigned 32-bit milliseconds, which rolls over
  every 49.7 days. Audio and video interleave and cross each other by a few
  milliseconds constantly, so at the instant the counter rolls, one track is
  over the line and the other is not - and the tag still carrying a pre-roll
  timestamp was read as if it belonged to the new cycle, landing it 49.7
  days in the future. The buffer trim measures everything it holds against
  the newest timestamp it has seen, so that one late audio tag put the
  cutoff past every frame in the ring and evicted the whole delay at once,
  dropping every viewer to live with no way back short of a restart. Only
  reachable by streaming without interruption for seven weeks, which is the
  unattended-relay case rather than the streamer one. Found by a new
  10,000-hour simulation, not in the wild.

- **A stream key or webhook URL with non-Latin characters crashed the app.**
  The redaction that hides a key on the dashboard counted bytes where it
  should have counted characters, so a value containing any multi-byte
  character (Japanese, Cyrillic, an emoji) cut one in half and took the whole
  process down. Because the value was already saved, it then crashed again on
  every dashboard load until the config was hand-edited. Present since the
  first release.
- **A recording path with an accent could stop the local sink from starting.**
  `instantclone sink --file <path>` prints the path in a fixed-width banner,
  shortening it to fit by counting bytes rather than characters. A path long
  enough to need shortening whose accent happened to straddle the cut took the
  process down before it ever listened. Same byte-versus-character mistake as
  the key redaction above; this was the last copy of it in the codebase. The
  banner's right-hand border now lines up for accented paths too, which it
  never had. Present since the sink shipped.
- **The overlay URL accepted more than a language code.** `/overlay?lang=`
  was written straight into the page it returns, so a crafted link could run
  script inside the overlay's page on InstantClone's own origin, which is
  enough to read your stream key or repoint a destination. The language is
  now checked against the list we actually have translations for, the same
  way the style parameter already was. Present since overlays shipped.
- **Video could be pushed into the delay without publishing first.** The
  stream key, and the optional ingest key with it, is checked when a client
  sends RTMP `publish`. A client that skipped that command and sent video
  frames anyway had them accepted, so the ingest key could be walked around
  and a second sender could interleave frames into your buffer. Media is now
  ignored until the connection has published. Only reachable with "listen on
  all interfaces" turned on. Present since the first release.
- **A stray early timestamp could empty the buffer at any moment.** The
  same 49.7-day arithmetic had a second way in that needed no waiting: a tag
  stamped before the session's first one has no earlier cycle to belong to,
  and was read as 49.7 days into the future instead, which the buffer trim
  then measured everything against. Reachable by anything that can publish
  to the ingest port, not only after seven weeks of uptime. The first tag of
  a session now sets the timeline and anything stamped before it is pinned
  to the present rather than launched past it.
- **Recording your first MIDI binding works on a fresh install.** Devices are
  only held open once something is bound to them, but the Learn button was
  checking whether a device was open rather than whether one was plugged in,
  so on a config with no bindings yet the two waited on each other and the
  first mapping could never be made. Caught by a pre-release review, not in
  the wild.
- **The tray drops its "Launch OBS (VOD + EB)" item.** It came from before
  the InstantClone service existed, when Enhanced Broadcasting on a custom
  RTMP server had to be switched on by launching OBS with a `--config-url`
  flag. The registered service now carries that configuration itself, so
  picking **InstantClone** as your service in OBS *is* Enhanced
  Broadcasting, with nothing to launch specially. The other half of the item
  wrote OBS's built-in VOD-track flag, which OBS 32.2 locked to Custom
  services - a VOD track comes from the unlocker script now. The item did
  neither job any more, so it is gone rather than misleading.
- **InstantClone no longer takes your MIDI controller hostage.** A MIDI
  input is exclusive on Windows: while one program holds it, nothing else
  can open it. The listener was opening every input device on the machine at
  startup whether or not anything was bound to one, so upgrading could have
  taken a controller away from the DAW or plugin host already using it. It
  now holds a device only while there is a binding to serve, or a learn
  waiting for a press, and hands it back when the last binding is cleared.
  Listing devices to choose from opens nothing, so the dropdown still fills.
- **A MIDI device could be selectable and never match.** The device name was
  cleaned one way when stored inside a binding and not at all when stored as
  the selected device, so a controller whose name contained the character
  that separates a signature from its device could be picked in the dropdown
  and then silently never fire. Every path now cleans the name identically,
  and the selected name is bounded like the one in a binding.

### Internal

- **The delay actions now build on every platform.** Toggle, arm, activate,
  cut and cut-after are plain state-machine code with no system call in
  them, but they were compiled only on Windows because the only things
  driving them (the tray's hotkey loop, the MIDI listener) are Windows-only.
  That left the Linux CI job compiling a different controller and skipping
  ten of the tests covering the newest logic. They are one clearly named
  block now, built and tested everywhere, with the platforms that have no
  driver yet declaring that rather than hiding it behind conditional
  compilation.

## [0.1.13] - Twitch VOD audio on the InstantClone service + per-destination audio routing

### Twitch VOD audio, unlocked with an optional script

OBS hardcodes its built-in **VOD Track** to the service literally named
"Twitch" (`ServiceSupportsVodTrack == {"Twitch"}`), so it was unavailable
while using the InstantClone service. A tiny optional OBS script now unlocks
it on *any* service, with no config spoofing and no plugin.

- **One-click VOD unlocker.** A new **System -> Behavior -> Twitch VOD audio
  unlocker** card (and the Twitch destination editor) downloads
  `optional-vod-unlocker.lua` straight into OBS's scripts folder. You add it
  once via **Tools -> Scripts**, and OBS starts sending a second audio track:
  the script attaches the same AAC encoder OBS's own VOD Track would, just
  without the service gate. **Required on OBS 32.2+** (that release locked the
  built-in VOD Track to Custom services); older OBS can still use the classic
  checkbox, and the dashboard steers you based on your detected OBS version.
- **Wizard rework.** The Twitch step now explains the script flow and no
  longer flips the legacy VOD-audio flag, so new setups don't trip the old
  IVS-override path.

### Per-destination audio track routing

Once OBS sends a second audio track, each destination picks what it receives.

- **Audio track selector.** Per destination: **Auto**, **Both** (Twitch VOD),
  **Track 1** (live) or **Track 2** (clean). Send the music-free track to
  YouTube to dodge copyright while Twitch keeps both, or the live track to
  Kick. Only Twitch consumes a second audio track, so every other platform
  always receives a single, flattened track (AAC is rewritten to legacy so
  every ingest accepts it), mirroring the existing video-side flatten. If the
  chosen track isn't being sent, the live track is used instead, so a
  destination never goes silent. Defaults to **Auto**, so existing
  destinations are unchanged.

### Fixes

- **Permanent "update available" nag.** The 0.1.12 build reported itself as
  0.1.11 (its package version was never bumped), so its update check flagged
  an update forever. The version now tracks the release and the About tab
  shows the real number.

## [0.1.12] - Kick RTMPS egress + per-streamer Server URL

### Kick now works (RTMPS egress)

Kick was never reachable before: the egress spoke plain RTMP on a hardcoded
host, but Kick ingests over **RTMPS** (TLS on :443) and hands each streamer
their own **Server URL** in the dashboard. Both are fixed.

- **RTMPS egress.** A destination whose URL is `rtmps://` now transparently
  upgrades the egress socket to TLS before the RTMP handshake, reusing the
  same `native-tls` (schannel) backend already linked for the HTTPS webhook
  client - no second TLS stack, ~120 KB of binary growth rather than the
  ~1.4 MB a rustls + ring stack would cost. Plain `rtmp://` destinations are
  unchanged.
- **Kick is a paste-your-Server-URL platform.** The destination form and the
  first-run wizard both show a **Kick Server URL** field (copied from the
  Kick dashboard: Settings -> Stream) alongside the stream key, validated
  and with a link to the dashboard. There is no hardcoded host, because the
  Server value is per-streamer. If you leave the `/app` path off, it is
  added for you (Kick's ingest app is always `app`).
- **Custom RTMP accepts `rtmps://` too.** Any TLS ingest (a non-standard
  Kick host, Facebook, etc.) works through the Custom platform now.
- **Simulcast is unaffected.** Kick over RTMPS runs alongside Twitch over
  plain RTMP on independent per-destination connections; a Twitch Enhanced
  Broadcasting ladder is still flattened to the single primary H.264 track
  for Kick automatically.

### Wizard fix

The OBS-wiring step showed a duplicate "I've set this up in OBS" button on
its optional Step 2 ("Check it's connected"), and the validation nudge that
highlights the confirm button pointed at that hidden Step 2 copy. Removed
the duplicate and moved the id onto the real Step 1 button, so the nudge now
draws the eye to the button you can actually see.

## [0.1.11] - Encoder compatibility warnings, Start with Windows, adaptive re-cut, and UX polish

### Encoder compatibility warnings

InstantClone now measures what OBS is actually sending and says something
only when it will bite one of your **enabled** destinations.

- **Keyframe interval** is measured from the IDR index (mean of the first
  5 gaps, then frozen for the session) and flagged above 2.6 s. OBS's
  "auto" typically lands at 4 s, which causes rebuffering on Kick and
  YouTube. The 0.6 s of headroom means a correctly-set 2 s stream, which
  measures ~2.0-2.1 s in practice, never trips it.
- **Resolution** is decoded from the SPS and checked against Kick's 1080p
  ceiling.
- **Codec** is flagged when HEVC/AV1/VP9 is heading for Kick's H.264-only
  ingest. Twitch and YouTube are deliberately excluded - Twitch negotiates
  codecs under Enhanced Broadcasting and YouTube has HEVC ingest paths, so
  warning about either would risk a false positive.

The design constraint was not adding clutter: **nothing renders when the
stream is healthy**, every problem collapses into a single line naming the
affected platforms (never per-check badges), disabled destinations and the
local sink are ignored, and the line is dismissible. Dismissal is keyed by
the message, so fixing one problem and hitting a different one brings the
strip back on its own. Custom RTMP gets no opinion at all - we have no idea
what a user's own ingest wants.

The measurement freezes once its sample budget is spent, on purpose. A
value that keeps drifting is what made the buffer-capacity gate strobe in
0.1.10, and a warning that flickers mid-stream is worse than no warning.

### Start with Windows

**System → Behavior → Start with Windows** registers InstantClone under the
per-user `Run` key (HKCU, so no elevation) with `--no-browser`, so it waits
in the tray at login instead of throwing a dashboard tab at you.

The registry entry is its own source of truth rather than a mirrored
setting: Windows gives users their own switches for startup entries (Task
Manager's Startup tab), and a stored copy of the flag would sit there
claiming "on" after someone turned it off elsewhere. Enabling always
rewrites the command, so an entry left behind by a previous install
location gets corrected instead of pointing at a missing executable.

### LAN exposure warning

The **LAN access** toggles now say what they actually do. The control API
has no authentication - the CSRF guard blocks cross-origin browser POSTs,
but a direct request (curl, Stream Deck) carries no `Origin` header and is
allowed through by design so CLI integrations keep working. On loopback,
the default, that is exactly right. Exposed to a LAN it means anyone on the
network can change the delay, cut, and edit destinations, and the ingest
port accepts any stream key. Both toggles remain off by default; the
warning appears only while one is ticked.

### Fixes

- The keyframe-interval sampler discarded the real first keyframe of every
  session. It used `first_idr_ts_ms == 0` as its "not started yet"
  sentinel, but an RTMP session normally starts at timestamp 0, so the
  window re-opened on the *second* keyframe and every subsequent mean was
  computed from the wrong origin. Caught by its own regression test before
  shipping; the window now has an explicit flag.
- The compatibility strip's dismissal is scoped to the publishing session:
  it clears when the stream stops, so a new session that hits the same
  problem warns again instead of inheriting a stale dismissal.
- The one-click updater now confirms before restarting while OBS is
  publishing, matching the tray Quit guard - an update tears down every
  egress the same way a quit does.

### Dependencies

- `tokio` 1.52.3 -> 1.53.0, `bytes` 1.12.0 -> 1.12.1.

## [0.1.10] - Improving visuals, adding "sink" test mode as destination, UX improvements and OBS dock

### Customizable OBS browser dock

A composable, per-slot browser dock so you can drive InstantClone from
inside OBS without opening the full dashboard - competitive players keep
the CPU free and only show what they need.

- **Widgets + presets** - status bar, delay number, buffer bar, egress
  glance, hint, destinations, delay profiles, health stats, auto
  behavior, live-safe settings, and an overlays picker. Toggle, reorder
  (drag), and restyle each from an in-dock editor. Presets range from
  **Delay only** to a full **Dashboard**.
- **Multiple docks** - `?dock=<id>` slots, each with its own saved
  layout, plus copy-URL flows to run a second dock in OBS. Layouts save
  server-side (survive OBS wiping its cache) and sync live across docks.
- **Destinations on the dock** - rows or icons, platform-coded colors,
  and a two-tap misclick guard on every toggle.
- **Safe cut on the dock** - schedule "cut after this airs" and watch a
  live, cancellable countdown; a cut scheduled anywhere shows on every
  dock.
- **Buffer-capacity gate** - a delay too big to fill at the current
  bitrate is greyed out and refused, with a tooltip naming the buffer
  size it needs. Enforced on the dock, the dashboard, and server-side on
  `/arm` so a stale page or scripted call can't stall in "arming".

### Fixes

- The first-run wizard could reappear after setup was already done (thanks
  **fashionxd** for the report). Two causes: (1) `configured` was recomputed
  live on every destination toggle/delete, so turning off or removing your
  last destination flipped it back to false and reopened the wizard - it is
  now a one-way first-run latch that only an explicit full reset clears;
  (2) every settings write did an unsynchronized clone -> mutate -> `send()`
  of the whole struct, so two overlapping POSTs lost one update (which could
  resurrect a stale `configured=false`). All settings mutations now
  serialize through a single process-wide write lock, closing the
  lost-update race for every field, not just this flag.
- The OBS dock showed a phantom **1.0s** delay in passthrough. It fed
  the big number from `current_delay_ms`, which includes the pipeline's
  own transit latency (encoder → ingest → ring → egress) even with no
  delay armed. The dock now mirrors the dashboard: the armed target
  while a delay is engaged (or the fill while arming), and 0 in
  passthrough.

### OBS quick launch from the System tab

The Enhanced Broadcasting launchers no longer hide inside the Twitch
destination editor. **System → Behavior → OBS quick launch** now offers:

- **Launch OBS · Enhanced Broadcasting** - starts OBS with the
  `--config-url` flag pointed at InstantClone, session-only.
- **Launch OBS · EB + VOD audio track** - same, plus it enables VOD
  audio mode on your enabled Twitch destination and writes OBS's
  VOD-track unlock flag, reporting each step as a red-to-green
  checklist. Persisting the destination flag matters: OBS's on-disk
  flag is derived from the destinations on every save, so a launch that
  skipped it would be silently reverted later.
- **Make a desktop shortcut** - the existing VOD+EB cold-start shortcut,
  now reachable without opening a destination.

The card warns inline when no enabled Twitch destination with a stream
key exists (OBS still launches, but EB has nothing to engage), and the
buttons disable with a reason when OBS isn't installed.

### Smooth aurora + hero text state transitions

The hero now changes state as one coordinated sweep - glow, number, and
pill move together - and the per-second stutter in the text and counter
is gone.

- The two aurora colors are registered CSS custom properties
  (`@property`), so a state change interpolates the colors themselves
  over ~1.1 s - the old `transition: background` never worked because
  gradients aren't transitionable as images. Browsers without
  `@property` keep the previous instant swap.
- The drift animation is now identical in every state. The old "arming"
  speed-up overrode `animation-duration` (14 s → 4 s) mid-flight, which
  remaps elapsed time and visibly teleported the blobs on every
  enter/exit of buffering. Arming's energy is a new dedicated breathing
  layer instead: a bottom glow that fades in over 0.9 s and pulses
  gently (transform only, so its restart never fights the fade).
- The big delay number's color rides the same 1.1 s sweep as the aurora
  (it used to snap a second ahead of the glow), and the state pill's
  text/background/border colors sweep too instead of hard-cutting.
- The subtext under the number only animates when the MESSAGE changes,
  not when the live numbers inside it tick (buffer fill, bitrate, and
  the auto-cut countdown update up to 4x/s - each tick used to replay
  the fade, a constant pulse). Message changes get a cleaner
  directional swap: the old line drops out, the new one drops in.
- The big delay number now actually animates - it never did. The shared
  `animateNumber` helper only stored per-element state on its animate
  path, which a fresh id could never reach (its throwaway object always
  had `to === target`), so the state map stayed empty and every number
  SNAPPED to each value. The counter has been a direct-set since it was
  written; the arming "stutter" and the "disarm 15s→0s with no
  animation" were both this. The helper now seeds its map on first
  sight, so the stat readouts ease properly too.
- The hero delay figure is driven by a dedicated critically-damped
  spring (SmoothDamp) on one persistent rAF, not a per-update tween. It
  tracks the buffer fill at smooth near-constant velocity while arming
  (no jerk from retargeting a varying gap over a fixed time) and eases
  the big jumps - notably the roll DOWN to 0 on cut / disarm - to rest
  with no overshoot, snap, or freeze, independent of update cadence.
- The hero delay-profile chips and the Profiles pane no longer flicker.
  Both rebuilt their whole DOM on every state tick (~4x/s); they now
  skip the rebuild unless the rendered content actually changed.
- The "delay too big for your buffer" gate is stable instead of jumpy.
  It planned from the raw instantaneous bitrate, so a momentary dip
  inflated the computed capacity and briefly unlocked a delay the
  buffer can't hold - and during that flicker the chip was clickable,
  arming a delay that could then never fill. Planning now uses a
  slow-decaying peak-hold of the bitrate (worst case), so the gate
  can't flicker or transiently unlock an unfillable delay.
- The "Adjusting → rewinding to 15s (currently 0s)…" line told a story
  the engine doesn't do - the delay never rewinds gradually; it waits
  for enough buffered history, then makes ONE IDR-aligned jump. The
  copy now says exactly that ("Building history - jumps to 15s once
  … buffered reaches it", then "Jumping back to 15s of delay…"), and
  the adjusting detector is gated on a live destination - with none
  connected, delivered delay reads 0 and the old line showed a fake
  permanent "rewinding" state.

### Local test sink destination

**Try the whole pipeline with zero risk.** The in-binary RTMP sink
(`instantclone sink`, until now a CLI-only tool) is available as a
destination: pick **Local test sink** in the Destinations tab and
InstantClone spawns its own tiny receiver on this PC and streams to it.
No platform account, no stream key, nothing leaves your machine - test
arm / activate / cut (and vertical) end to end before touching a real
key.

- The managed child process follows the destination's lifecycle: spawned
  when a sink destination is enabled (one child serves any number of
  them), killed when the last one is disabled or removed, reaped and
  respawned if it dies, and taken down with the app. It lives on fixed
  high ports (rtmp :19350, player :19351) so manual `instantclone sink`
  runs on the documented defaults (:1936/:1937) never collide with it.
- The destination card gets a **▶ Watch output** link to the sink's
  built-in live player - watch exactly what a platform would receive,
  delay and cuts included.
- The sink's own log lines (`publish accepted`, the 1 Hz stat windows)
  are forwarded into the dashboard's Logs tab, so the feedback loop is
  visible without a terminal.
- The destination form hides the stream-key field for it (any key works)
  and explains the feature inline; the first-run wizard deliberately
  does not offer it - the wizard wires your real platform.

### "Cut after this airs" - scheduled safe cut

**Stop counting your delay down in your head.** With a delay active, a new
**⏱ Cut after this airs** button appears under Cut in the dashboard. Press
it the moment your match reaction ends: InstantClone records that exact
live-edge timestamp, lets everything up to it reach your viewers, then
fires the normal IDR-aligned cut to live on its own. Built for competitive
streamers who finish a match on a 30 s delay and want the win/lose
reaction to air in full before snapping back - without the mental
countdown or the risk of clipping the reaction short.

How it behaves:

- The cut fires only when the **slowest** live destination has aired past
  the mark, so a faster destination can never cut a slower one short of
  it. Exactly one pump fires the cut (compare-exchange), and it rides the
  same IDR-aligned, connection-preserving machinery as a manual Cut - the
  new e2e scenario F verifies the downstream connection survives.
- While the mark is pending, the button becomes a live countdown
  ("auto-cut in ~27s"); tapping it again cancels without cutting. A
  manual Cut, a disarm, or a fresh OBS publish session all clear the
  mark (a mark on a dead session's timeline could never be reached).
- New HTTP endpoints: `POST /cut-after` (409 when no delay is active) and
  `POST /cut-after/cancel`; `/state` gains `safe_cut_pending` +
  `safe_cut_remaining_ms`.
- The setup wizard and the onboarding tour both explain the flow, and the
  button carries a hovercard with the press → airs → auto-cut timeline.

### Tests

- Added a unit suite for the RTMP chunk-stream layer (`rtmp/chunk.rs`),
  previously covered only by the end-to-end job. 15 new cases exercise the
  reader's fmt-1 / fmt-2 / fmt-3 decode paths (which the writer never emits
  but OBS and Twitch send constantly), extended-timestamp encode+decode
  including across fragmentation, in-band Set-Chunk-Size and Window-Ack
  control handling, basic-header CSID range encoding, and malformed-input
  guards (truncated header, truncated payload, zero-length message) that
  assert an error is returned rather than a panic.
- Added pure-function tests for `trace::hex_prefix` (formatting, boundary,
  and truncation-suffix behaviour).
- Added 5 controller tests for the scheduled safe cut: refusal without an
  active delay, schedule/cancel round-trip, slowest-consumer firing gate,
  clearing on manual cut / disarm, and clearing on publisher reconnect.
- New e2e scenario F drives `/cut-after` against a live ffmpeg stream:
  409 refusal, cancel-without-cutting, auto-fire back to passthrough, and
  the sink's connection surviving the auto-cut.
- Test count: 210 -> 234, all green.

## [0.1.9] - Vertical output + VOD/EB convenience

### Vertical output for non-Twitch destinations (YouTube Shorts, Kick mobile, TikTok)

**Reuse Twitch Dual Format's vertical canvas to simulcast 9:16 anywhere,
with zero extra encoding.** When you enable Twitch Dual Format (Enhanced
Broadcasting) in OBS, OBS already produces a vertical 9:16 canvas and
sends it to InstantClone alongside the horizontal one. Until now every
non-Twitch destination only ever got the horizontal primary track and the
vertical canvas was discarded. Each destination now has a **Stream format**
choice (Horizontal / Vertical); set a YouTube, Kick, or custom destination
to **Vertical** and it forwards the vertical canvas instead, flattened to a
standard single-track 9:16 RTMP feed those platforms accept natively (this
is exactly YouTube's "Dual stream, separate encoder key" path - paste the
vertical stream key).

How it stays robust:

- The vertical canvas is identified by **decoding each track's SPS for
  orientation** (portrait = `height > width`), not by guessing track IDs -
  the canvas-to-track mapping lives only in Twitch's private session JSON,
  so we read what's actually on the wire instead. The SPS parser is fully
  bounds-checked and never panics on partial or hostile bytes.

- A vertical destination whose canvas isn't available yet (Dual Format
  off) **waits and sends no video**, showing "Waiting for Dual Format",
  while Twitch and every horizontal destination keep streaming untouched.
  Detection re-runs each supervisor tick, so turning Dual Format on/off
  mid-stream self-heals with no restart.

- The control is **hidden for Twitch**, which sends both canvases natively
  via Dual Format - a note explains that so the choice never confuses.

- Each destination card shows the **detected resolution + codec** of the
  track it forwards (e.g. `1080x1920 · H.264`), so you can confirm the 9:16
  canvas is really flowing - and a **Dual Format** header pill lights up
  whenever a vertical canvas is on the wire.

- The vertical AVC feed is forwarded as **legacy AVC framing**, matching the
  horizontal primary. YouTube's vertical ingest is viewable but flaky with
  Enhanced-RTMP `avc1` (it aborts the connection on a ~11 s cycle); rewriting
  the vertical track to legacy AVC keeps the connection stable. A vertical
  destination also **leads with its own keyframe** after every (re)connect or
  cut, so viewers get a clean picture instead of a mid-GOP glitch.

### Clearer vertical / Dual Format status in the dashboard

- The **Stream format** picker now **disables Vertical when no enabled Twitch
  destination exists** (with an inline reason + a link to Twitch's guide).
  Vertical has no source without Twitch Enhanced Broadcasting, so the choice
  is no longer a silent dead-end. A destination already set to vertical is
  never force-flipped - the option stays available in case its source returns.

- A waiting vertical destination now says **why** it isn't live instead of a
  generic "waiting": **Needs Twitch EB** (no Twitch source at all), **No
  vertical canvas** (Enhanced Broadcasting is live but the 9:16 canvas isn't
  on the wire), or **Waiting for Dual Format** (nothing streaming yet). The
  copy makes clear Dual Format is **Enhanced Broadcasting + Aitum Vertical**
  (per Twitch's guide), not a single OBS toggle, so the setup path is honest.

- The per-destination status endpoint now parses each cached video sequence
  header **once per poll** instead of once per destination - less work on the
  frequently-polled dashboard path.

### VOD audio + Enhanced Broadcasting: fewer manual steps

- New **one-click "Set up VOD + EB"** writes OBS's VOD-track unlock flag,
  launches OBS with the `--config-url` flag (the only path OBS honours for
  Custom RTMP), and re-verifies the flag landed, reporting a red-to-green
  checklist with the exact fix when a step fails (partial-success aware,
  e.g. "close OBS and retry").

- New **"Create desktop shortcut"** generates a Desktop `.lnk` (with a
  `.cmd` fallback) that cold-starts InstantClone in VOD + EB mode via the
  new `--launch-eb` flag - one double-click brings up the whole setup with
  no dashboard clicks. The same one-click launch is available from the
  tray ("Launch OBS (VOD + EB)").

## [0.1.8] - VOD-audio session race fix

**VOD audio could go live Source-Only with duplicate sessions.** On a
stream with VOD audio enabled, Twitch Inspector sometimes showed several
short sessions for a single go-live, the first one Transmuxed
(Source-Only) instead of Transcoded, and viewers hit a playback error.

Root cause: the egress supervisor wakes every ~2s and, for a Twitch
destination with VOD audio on, asks Twitch's API to allocate an IVS
session, then points egress at the URL it returns. The old code fired a
fresh GetClientConfiguration call on every tick until one came back. When
Twitch answered slower than the 2s tick, the calls stacked. Each one
allocated its own IVS session and rewrote the destination's override URL,
and each rewrite restarted egress, so a single OBS stream reached Twitch
as a string of brief sessions. Whichever session won the race decided
whether the stream came up Transcoded or Source-Only.

Two guards, both per-destination so multistreaming is unaffected:

- A single-flight latch keeps exactly one session fetch in flight at a
  time. It re-checks the override under its lock, so a fetch that just
  finished can't let a duplicate slip in right behind it.

- A session epoch, bumped on every publisher disconnect, records which
  publisher session a fetch belongs to. A request that returns after OBS
  disconnected, whose IVS token is now bound to a dead session, is
  discarded instead of being written into the next stream. The apply check
  and the disconnect both touch the override under the same lock, so they
  cannot interleave.

The `/obs/multitrack-config` proxy path (Enhanced Broadcasting via the
registered OBS service, and the experimental VOD+EB Launch button) was
never affected: it runs inside OBS's blocking config request, before the
stream starts, so it always sets the override for the session about to
begin. Non-Twitch destinations do not use this path at all.

## [0.1.7] - Tour + VOD audio UX fixes

**App tour overlay step updated.** The tour bubble for the Overlay tab
said "Picks one of 6 built-in overlay styles" - that was the old UI
before Overlay Studio shipped. It now describes what's actually there:
browse the gallery, preview overlays in any delay state (Idle / Arming /
Ready / Active), copy the URL into OBS as a Browser Source, and hit
**+ New overlay** to open Overlay Studio for a full visual editor.
Hand-written `.html` files in the overlays folder still show up
automatically.

**VOD audio has a setup guide and a clearer warning.** Enabling VOD
audio requires switching OBS to Custom RTMP mode first, but that step
was easy to miss. A collapsible "How to set this up in OBS" guide now
sits inside the toggle - five steps covering Service: Custom..., ingest
URL from the OBS tab, Advanced output mode, the VOD Track checkbox under
Streaming, and the apply-then-restart sequence. A separate warning line
also flags that OBS must be closed before saving: the config file can't
be updated while OBS holds it open, so the setting won't take effect
until the next OBS launch.

## [0.1.6] - Overlay flow: one list, copy a URL

Reworks the Overlay tab around a single idea: **everything is just an
overlay with a URL you copy into OBS.**

**One list, presets pre-installed.** The old preset / editable / legacy
split (and its type filter) is gone. On first run the built-in presets
are baked to real overlay files, so they show up in the same list as
anything you make - each with a working URL, no hidden "materialize on
copy" step. A one-time `overlays_seeded` flag keeps deleted presets from
reappearing.

**Three real actions.** Per overlay: **Copy URL**, **Edit in Studio**,
**Duplicate**, **Delete** (hand-written legacy files: Copy URL + Delete).
The standalone "Open Studio" button is replaced by **+ New overlay**. In
the Studio, save is now two clear buttons: **Save** (overwrite) and
**Save as new**.

**Auto-hide as a quick toggle.** A per-overlay **Auto-hide when live**
switch; turning it off appends `?autohide=off` to the copied URL so the
overlay stays up, no re-bake. The per-state control in the Studio remains
the baked default.

**Restore defaults.** A button (and the factory reset) wipes your Studio
overlays and reinstalls the built-in set, with a confirmation that says
so plainly. Legacy hand-written files are left untouched.

**Legacy overlays retired.** The bundled `minimal` / `corner` / `strip`
hand-written overlays are removed (superseded by the Studio presets); the
startup no longer drops them. Serving still works - any `.html` you put in
`overlays/` is served, and the older `/overlay?style=…` URLs still render.
The `custom-template.html` hand-write example stays.

**Calmer graph meter.** The line widget now smooths its signal (a
**Smoothing** slider, on by default) and floors its vertical scale, so a
steady stream reads as a calm line instead of amplifying tiny jitter into
a full-height sawtooth - while a real drop still swings the whole graph.

**One-click updates.** When the About tab finds a newer release, an
**Update** button now does it for you: download the new build, verify it
against the release's published SHA-256, swap it in, and relaunch - your
settings, destinations, and overlays are kept. Updating stays optional,
and the manual download link is still there for anyone who prefers it (or
when antivirus blocks the swap). Saving settings also shows an in-flight
state and reports network failures instead of silently doing nothing.

**Effects you can tune.** Liquid fill gained real controls - **Fill level**
(follow the buffer or a fixed %), **Wave**, **Speed**, and **Smoothness** -
instead of a hardwired behavior. Heartbeat gained **Speed**; the buffer bar
and ring gained **Smoothness**. Untuned widgets look exactly as before.

**Studio preview stays in sync.** Editing a setting now re-arms all auto-hide
timers together immediately (a brief "synced" tag confirms it), instead of
drifting until the next preview cycle. The **theme colour picker** also no
longer slams shut the moment you drag the hue.

**Auto-hide exits actually animate.** When a widget hid after its delay, it
used to just blink out regardless of the chosen exit (fade / slide / pop):
the browser collapsed the animation-stop and the fade into one frame and
skipped the transition. The exit now plays its full animation. The Studio
preview also adapts its replay timing so a hide longer than 5 s plays its
exit instead of looking like it never fires.

**Unsaved overlay changes are guarded.** The Studio's Save button shows a dot
when there are unsaved edits, and leaving (Back, switching tabs, or closing
the page) now asks before discarding them.

**Restart from the dashboard.** The "restart required" banner (and the
About tab) now has a **Restart now** button that relaunches the app and
reconnects the dashboard on its own, instead of asking you to find and
relaunch the exe by hand. **Show in Explorer** buttons on the Buffer and
Diagnostics sections open the buffer file, trace log, and overlays folder
straight in Explorer for troubleshooting.

## [0.1.5] - Overlay Studio (experimental)

A no-code visual editor for building OBS browser-source overlays that
react to the delay state machine, replacing the old fixed style-picker
on the Overlay tab.

**Experimental, and isolated by design.** The studio is new, so rough
edges and bugs are expected *in the studio itself*. It cannot affect a
live broadcast: the delay proxy, buffer, and SSE state stream are
untouched, and overlays only ever run as an OBS browser-source. The
hand-written overlays (`minimal`, `corner`, `strip`) still serve
verbatim, so anyone who wants zero overhead can ignore the studio, pick
a minimal preset, or keep their own static HTML.

**Author rich, bake to lean static.** A shared JS runtime renders the
editor canvas, then *bakes* each overlay to a self-contained static HTML
file with only the CSS and JS it actually uses inlined. `/overlay/<slug>`
serves that static artifact, so a baked overlay costs about what a
hand-written one does at stream time, with no shared-runtime fetch.

**Widgets, states, animations.** 21 ready-made widgets (delay readout,
state pill, buffer bar/ring, graph meter, destination status, heartbeat,
liquid fill, edge accent, corner frame, banner, image, and more). Every
property (colour, opacity, position, size, font, gradient, glow,
animation) is editable per delay-state (idle / arming / ready / active /
error). 13 compositor-only animations, each with an adjustable intensity,
plus gradients (including animated) and glow.

**Presets.** 10 built-in presets across minimal / casual / ambient /
tournament / technical / power styles. Editing forks to a copy; the
originals stay immutable. Presets fade themselves out 4s after going
live so the overlay clears once viewers have registered the delay; a
quick toggle on the Overlay tab keeps it up instead, or tune it per
state in the Studio. Appending `?autohide=off` to the browser-source
URL also keeps any overlay up, no re-save needed.

**CustomHTML escape hatch.** A sandboxed widget with live data exposed
two ways: `{{template}}` vars for non-coders and a `window.ic` JS API
(`ic.phase`, `ic.delay`, `ic.onUpdate(...)`) for power users.

Studio overlays are stored as `<slug>.json` (editable source) plus the
baked `<slug>.html`; both are served from `overlays_dir`. Saving, slug
sanitization, and path-traversal containment are covered by new tests.

## [0.1.4] - Auto-arm + auto-activate behavior toggles

Two independent System settings, both default off, for streamers who
don't want to remember the manual arm/activate ceremony every
session. The deliberate two-phase flow stays canonical for everyone
else.

**Auto-arm on OBS connect.** When the publisher handshake completes,
the supervisor detects the false-to-true edge on `ingest_alive` and
fires `arm_delay(auto_arm_delay_ms)`. Skipped if the streamer
manually disarmed earlier in the same session (since `armed_delay_ms`
stays at 0 until the next publisher reconnect). Designed for
tournament players + IRL streamers who always want a safety buffer.

**Auto-activate when buffer ready.** Independent toggle. The
supervisor detects the phase transition into `ready` and fires
`activate_delay()`. With both toggles on, every OBS-connect becomes
a zero-touch path to live-with-delay. With only auto-activate on,
every manual arm becomes one-step. Loses the "click when I'm ready"
deliberate moment but matches casual-stream UX.

**Default delay.** New `auto_arm_delay_ms` field tracks the last
value the streamer manually armed at, so "default" matches habit
without a separate UI to maintain it. `persist_delay_state` writes
this on every non-zero arm; Disarm (`arm_delay(0)`) preserves the
preference. Default 15s on fresh install. `sanitize_load` clamps to
the same 10-min ceiling as the other delay fields and replaces 0
with 15s so a hand-edited config never auto-arms at "0 seconds of
delay" (which would be a no-op + confusing).

**System tab UI.** New "Behavior" section sits at the top of the
existing System tab grid: two checkboxes + one number input,
descriptions below each. Step 1 of the planned multi-step System
tab redesign; the rest follows in v0.1.6.

**Tests added: 154 -> 160.** Six new regressions in `config::tests`
and `web::tests` covering the `apply_field_str` dispatch path
(post_config's whitelist + the match arm), the save/load round-trip
(conditional emit + parse path), the config-lean default-omission,
and the sanitize-load clamp + zero-fallback. Modeled after the
existing `tracing_enabled` regression which caught the same class of
bug between beta builds.

## [0.1.3] - VOD audio flag actually works (issue #9)

**VOD audio toggle did nothing on OBS 32.** Reported as issue #9
on v0.1.2: the streamer toggled VOD audio in InstantClone, restarted
OBS, and the "Twitch VOD audio track" option stayed locked - both
with the InstantClone-registered service AND with Custom RTMP.

Root cause: OBS 32 split the legacy `global.ini` into two files -
`global.ini` (app-level) and `user.ini` (user-level) - and the
VOD-track frontend gate reads `App()->GetUserConfig()` which now
maps to `user.ini`:

    bool enableForCustomServer = config_get_bool(
        App()->GetUserConfig(), "General", "EnableCustomServerVodTrack");
    bool enableVodTrack = ui->service->currentText() == "Twitch";
    if (enableForCustomServer && IsCustomService())
        enableVodTrack = true;

We were writing the flag to `global.ini`, which OBS 32 simply
ignored. v0.1.0..0.1.2 shipped the toggle as a no-op without anyone
catching it because we never tested against a fresh OBS 32 install
with a real Custom RTMP service.

Fix: a new `obs_user_config_path()` prefers `user.ini` when present
(OBS 32+) and falls back to `global.ini` for older installs that
haven't been split yet. The active-profile detection now resolves
through the same helper, so both the VOD flag and the `[Basic]
Profile=` lookup follow the same source of truth. Every write to
`set_vod_audio_flag` also flips the same key to `false` in the
now-orphan `global.ini`, so users upgrading from v0.1.0..0.1.2 end
up with no live flag anywhere. Pure path-resolution helpers
`resolve_user_config_in` and `legacy_global_ini_in` are unit-tested
against a temp directory so the prefer-user.ini behaviour can't
silently regress.

**Toggle-OFF now flips the value in place instead of deleting the
line.** On the v0.1.3 test build, OBS 32 was observed rewriting
`user.ini` on shutdown and consolidating its own duplicate
`[General]` sections - which would have left a "deleted by us" path
fragile against where exactly OBS chose to keep the key after the
rewrite. New semantics: `set_vod_audio_flag(true)` writes
`EnableCustomServerVodTrack=true`, `set_vod_audio_flag(false)` writes
`EnableCustomServerVodTrack=false`, and we never try to find-and-
delete. OBS's `config_get_bool` reads `=false` and "key absent"
identically, so the runtime behaviour is the same; the robustness
gain is that wherever OBS shuffles the key to between sessions, the
next toggle just flips the value in place. Insertion is still gated
to the enable path, so a never-toggled `user.ini` stays clean.
Tests `ini_set_writes_false_in_place_when_disabling`,
`ini_set_does_not_create_false_line_when_disabling_a_virgin_file`,
and `ini_set_flips_value_to_false_in_trailing_duplicate_general_section`
lock in this behaviour against the exact file shape OBS 32 produces.

**Stopped swallowing reconcile errors.** `reconcile_obs_vod_files`
previously used `let _ = ...` on every write call. When OBS was open
and held its files locked, `write_or_friendly` returned
`PermissionDenied`, the toggle silently failed, and the user got no
feedback. The function now takes the controller and logs failures
to the dashboard event log with the message
`vod-audio: couldn't write OBS user config (...). Close OBS, then
toggle the destination off and back on to retry.` so the path
forward is obvious from the Logs tab.

**Enhanced-RTMP audio wire format read correctly for the first time.**
The fundamental bug behind issue #9's "both tracks silent" symptom
on manual test: v0.1.0..0.1.3 mis-located the `AudioPacketType` and
`TrackId` fields in Enhanced-RTMP audio tags. The classifier read
`PacketType` from `payload[1] & 0x0F` and `TrackId` from `payload[7]`;
the correct positions per OBS's
`plugins/obs-outputs/flv-mux.c::flv_packet_audio_ex` are
`payload[0] & 0x0F` (high nibble of byte 0 is `SoundFormat=9`, low
nibble is the packet type) and `payload[6]` (after the multitrack
header byte and the 4-byte FourCC).

The user-visible failure: when OBS sends the Live track as
Enhanced-RTMP single-track (which it does whenever VOD audio is
enabled, since the encoder switches to the EX header path for
idx=0 even without multitrack wrapping), our classifier saw
`PacketType = 'm' & 0x0F = 0x0D` and never matched
`SequenceStart=0`. The `AudioSpecificConfig` tag flowed into the
ring buffer like a regular audio frame, got evicted by `trim_older_than`
once newer tags accumulated past the delay window, and the
destination consumer connected to Twitch with no decoder config
ever sent. Twitch's `Inspector` still listed both tracks because
OBS's `onMetaData` declared them, but the actual frames were
undecodable - both tracks silent.

Fix: corrected `classify_audio_tag`, `select_audio_bytes`, and
`audio_seq_header_track_id` to read the real byte positions. Test
data builders (`enhanced_audio_onetrack`, new
`enhanced_audio_single_track`) now emit byte sequences identical
to what OBS actually puts on the wire, verified against
`flv_packet_audio_ex`. Two existing tests that asserted the broken
interpretation (`enhanced_rtmp_opus_audio_recognised`,
`enhanced_aac_seq_header_via_packet_type_0`) were rewritten to
match the spec layout. New test
`enhanced_single_track_seq_header_is_detected` locks in the exact
bytes that triggered the regression so a future refactor can't
silently re-introduce the off-by-one.

**Audio multi-track passthrough decoupled from Enhanced Broadcasting.**
Reported during manual test of the v0.1.3 build: OBS was sending
VOD audio (mixer track 2, wire TrackId 1) but Twitch was receiving
both Live and VOD tracks silent. Root cause was a flag-scope bug
in v0.1.0..0.1.2: `pass_through_multitrack_video` gated both audio
AND video selection, and was only set true when an EB session was
allocated. For a Twitch destination without EB but with VOD audio:
TrackId 0 (live) was forwarded wrapped in Enhanced-RTMP multi-track
framing that Twitch's regular ingest can't decode, and TrackId 1
(VOD) was dropped entirely. New `pass_through_multitrack_audio`
flag is decoupled and set to true for every enabled Twitch
destination - VOD audio has worked on Twitch's regular ingest for
years, predating EB. Non-Twitch destinations still drop TrackId
!= 0 so a simulcast YouTube / Kick doesn't choke on a track its
decoder can't map. `send_sequence_headers` also gates the audio
seq-header replay on the same audio flag now, so a VOD track's
AudioSpecificConfig gets re-emitted on egress restart for any
Twitch destination, not only EB ones.

**Phase C (experimental "EB on Custom RTMP") replaced with a "Launch
OBS for EB + VOD" button.** After confirming the file-injection path
was architecturally dead (OBS's `rtmp_custom` plugin only declares
five known settings keys in `rtmp_custom_update` and discards every
other field at LOAD time, never at SAVE - so our injected
`multitrack_video_configuration_url` was never read at all),
re-implemented Phase C using the only path OBS's frontend actually
honours for Custom RTMP: the `--config-url` command-line argument
(`frontend/utility/GoLiveAPI_Network.cpp::MultitrackVideoAutoConfigURL`
checks the CLI flag before consulting the service settings object).
The dashboard's destination editor now shows a "Launch OBS for EB +
VOD" button under the VOD audio toggle when the user enables VOD
audio on a Twitch destination. The button hits a new
`POST /obs/launch-with-eb` endpoint that spawns
`C:\Program Files\obs-studio\bin\64bit\obs64.exe --config-url <our
endpoint>` (detached process group so closing InstantClone afterwards
doesn't take OBS down). The flag is per-launch, so the UI copy makes
it clear the user has to use this button every time they want EB on
top of VOD audio. The old `inject_vod_eb` file-edit path and its UI
sub-toggle are gone; the legacy strip is preserved and runs on every
reconcile so users upgrading from v0.1.0..0.1.2 end up with a clean
service.json. Recommended paths for the simpler cases stay the same:
- EB alone -> register InstantClone as an OBS service
- VOD audio alone -> Custom RTMP + the per-destination VOD toggle
- Both together -> the new Launch button (experimental)

**EB cuts no longer pixel-glitch on the legacy primary track.**
Confirmed via trace log inspection during the v0.1.3 EB+VOD manual
test: `compute_delay_cut` was landing on whichever ladder rung's IDR
happened to win the partition_point in the IDR-only secondary index,
including TrackId-1..4. The destination's decoder for the LEGACY
primary track (the one Twitch's transcoder anchors on) then received
a P-frame with no reference, producing visible pixel artefacts until
the next track-0 IDR ~2 s later. Fix: new `is_primary_video_idr`
classifier in `h264.rs` and a gate in `Ring::append` that only adds
the PRIMARY track's keyframes to `idr_index`. Multi-track ladder
IDRs still live in the main `index` (forwarded bit-faithfully to
Twitch IVS) - they just stop being cut candidates. Since OBS aligns
every ladder rung's IDR to the same encoder PTS, no cut points are
lost; we just stop landing on a rung where the legacy decoder has
no anchor. Six unit tests in `h264::tests` lock in the classifier
(legacy AVC keyframe accepted, legacy inter rejected, OneTrack
TrackId=0 accepted, OneTrack TrackId 1..4 rejected, Enhanced-RTMP
single-track P-frame rejected, truncated multi-track handled
gracefully). Three pre-existing buffer/controller tests were updated
to seed proper primary-IDR payloads via a new `primary_idr_payload`
helper - they previously used `[0u8; N]` which incidentally passed
through the old "any-track" index but is correctly rejected now.

**Dashboard title contrast.** `.ic-brand-mark` is now a solid warm
cream (`#f5efe1`) instead of the previous white -> 30%-accent
gradient. The bottom half of the letters was muddied to ~30%
lightness on most themes, which read as low-contrast against the
dark header.

**Passive update check.** New `src/update_check.rs` calls GitHub's
Releases API once per process lifetime (with a 10-minute cache to
avoid hammering on dashboard refreshes), parses `tag_name`, and
compares against the compiled-in `CARGO_PKG_VERSION`. A small "v0.1.4
available" pill appears in the dashboard header on load when a newer
release is published. Failures are silent - offline users and
GitHub rate-limit hits don't get an error toast, just no pill. The
SemVer-ish comparison correctly handles prerelease suffixes
(`0.1.3` beats `0.1.3-beta.7`) so users on a beta tag don't get an
"update available" prompt that points back at their current build.
Six unit tests in `update_check::tests` lock in the tag-name parser
and the version comparator.

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

[Unreleased]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.14...HEAD
[0.1.14]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.13...v0.1.14
[0.1.13]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.12...v0.1.13
[0.1.12]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.11...v0.1.12
[0.1.11]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.10...v0.1.11
[0.1.10]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.9...v0.1.10
[0.1.9]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/Soulhackzlol/InstantClone/compare/v0.1.0...v0.1.6
[0.1.0]: https://github.com/Soulhackzlol/InstantClone/releases/tag/v0.1.0
