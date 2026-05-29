# Contributing

Thanks for looking. A few quick notes so we don't waste each other's time.

## Filing a bug

Use the [bug report template](.github/ISSUE_TEMPLATE/bug_report.yml). The more
concrete you can be, the faster I can fix it: InstantClone version, Windows
version, OBS version, what you did, what you expected, what you got. Logs from
the dashboard's **Logs** tab go a long way (redact stream keys if any leak in).

## Asking a question

Open a [Discussion](https://github.com/Soulhackzlol/InstantClone/discussions),
not an issue. Issues are for things that need fixing.

## Sending a PR

- Keep PRs focused. One change per PR is easier to review than a grab bag.
- Run the test suite and make sure `cargo clippy --release` is clean before
  you push: `cargo test --release` should print `73 passed; 0 failed`.
- Match the existing code style. `cargo fmt` handles most of it.
- Mention how you tested. "Tested against OBS 30.2 streaming to a local
  RTMP sink for ~5 min with a 15 s armed delay" beats "works on my machine".
- New features: consider opening an issue first so we don't both write the
  same thing.

## What I'm interested in

- Cross-platform support (macOS, Linux). The code is mostly portable;
  `tray.rs`, `portcheck.rs`, and `sysstat.rs` have Windows-specific paths
  that need parallel implementations.
- More overlay styles. Drop new `.html` files into `overlays/`, they're
  picked up automatically.
- Real-world bug reports from streaming with the proxy in the loop.

## What I'm less interested in

- Refactors with no behaviour change. The code is small enough.
- Wide-net dep additions. Runtime stays at `tokio` + `bytes` (+ `windows-sys`
  on Windows) unless there's a strong reason.

## Build

Rust **1.74+** stable.

```powershell
cargo build --release
cargo test --release
.\target\release\instantclone.exe
```

## Heads-up

I'm one person, weekends mostly. Patience appreciated. If something's been
sitting for a week without a reply, a friendly bump is fine.
