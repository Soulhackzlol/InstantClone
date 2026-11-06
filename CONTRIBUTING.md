# Contributing

Hey, thanks for stopping by. Quick rundown so you know what to expect.

## Filing a bug

Use the [bug report template](.github/ISSUE_TEMPLATE/bug_report.yml). The more concrete the better: InstantClone version, Windows version, OBS version, what you did, what you expected, what actually happened. Logs from the dashboard's **Logs** tab help a ton (redact stream keys if any leak in).

## Asking a question

Open a [Discussion](https://github.com/Soulhackzlol/InstantClone/discussions). Issues are for things that need fixing.

## Sending a PR

- One change per PR makes review easier.
- Before pushing: `node scripts/check.mjs` runs what CI runs (format, lint, the Rust tests and the page tests) in one terminal, and has to end with every check passed.
- A line on how you tested goes a long way. "Tested with OBS 30.2 against a local RTMP sink for ~5 min with a 15 s armed delay" beats "works on my machine".
- For new features, open an issue first so we don't duplicate effort.

## Stuff I'd love help with

- Cross-platform support (macOS, Linux). The code is mostly portable; [tray.rs](src/tray.rs), [portcheck.rs](src/portcheck.rs), and [sysstat.rs](src/sysstat.rs) have Windows-specific paths that need parallel implementations.
- New overlay presets + Studio widgets. Built-in overlays are the `PRESETS` in [web/overlay-runtime.js](web/overlay-runtime.js) (the dashboard seeds them on first run); new widgets/animations live in the same file. A hand-written `.html` dropped into [overlays/](overlays/) is still served as-is.
- Real-world bug reports from streaming with the proxy in the loop.

## Probably not

- Pure-refactor PRs with no behaviour change. The codebase is small; churn costs more than it earns.
- Wide-net dep additions. Runtime stays at `tokio` + `bytes` + `ureq` (+ `windows-sys` on Windows) unless there's a strong reason.

## Build

Rust 1.74+ stable.

```powershell
cargo build --release
cargo test --release
.\target\release\instantclone.exe
```

The dashboard page is kept as pieces under [web/dashboard/](web/dashboard/), one folder per feature (`destinations/`, `wizard/`, `system/`…, with `page/` for what the whole page shares). [web/dashboard/index.html](web/dashboard/index.html) is its skeleton: the page's own tags with a `<!-- include hero/hero.css -->` line where each piece goes, in the order the page needs (styles in cascade order, markup in on-screen order, scripts in run order). `build.rs` swaps each include line for its piece. The Integrations script (`/integrations.js`) works the same way from [web/integrations/integrations.js](web/integrations/integrations.js). A new piece needs its include line in the skeleton, or the build stops and says so.

## Be decent

Participation here is covered by the [Code of Conduct](CODE_OF_CONDUCT.md). Short version: be respectful, no harassment.

## One last thing

I'm one person doing this on weekends. If a PR or issue sits for a week or two without a reply, a friendly bump is welcome.
