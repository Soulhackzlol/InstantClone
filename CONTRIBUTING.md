# Contributing

Hey, thanks for stopping by. Quick rundown so you know what to expect.

## Filing a bug

Use the [bug report template](.github/ISSUE_TEMPLATE/bug_report.yml). The more concrete the better: InstantClone version, Windows version, OBS version, what you did, what you expected, what actually happened. Logs from the dashboard's **Logs** tab help a ton (redact stream keys if any leak in).

## Asking a question

Open a [Discussion](https://github.com/Soulhackzlol/InstantClone/discussions). Issues are for things that need fixing.

## Sending a PR

- One change per PR makes review easier.
- Before pushing: `cargo fmt`, `cargo clippy --release` clean, `cargo test --release` showing `154 passed; 0 failed` (or higher if the suite has grown since this line was last touched).
- A line on how you tested goes a long way. "Tested with OBS 30.2 against a local RTMP sink for ~5 min with a 15 s armed delay" beats "works on my machine".
- For new features, open an issue first so we don't duplicate effort.

## Stuff I'd love help with

- Cross-platform support (macOS, Linux). The code is mostly portable; [tray.rs](src/tray.rs), [portcheck.rs](src/portcheck.rs), and [sysstat.rs](src/sysstat.rs) have Windows-specific paths that need parallel implementations.
- New overlay styles. Drop new `.html` files into [overlays/](overlays/) and they're served automatically.
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

## One last thing

I'm one person doing this on weekends. If a PR or issue sits for a week or two without a reply, a friendly bump is welcome.
