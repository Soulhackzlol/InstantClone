# Security

If you find something with security implications in InstantClone, please
**don't open a public issue.** Two private routes:

- [Open a private security advisory on GitHub](https://github.com/Soulhackzlol/InstantClone/security/advisories/new)
  (preferred, gets routed to me directly).
- Email me through the contact form on [s1moscs.dev](https://s1moscs.dev).

I'll respond within a few days, confirm whether it's a real issue, and work
on a fix before any public disclosure.

## What's in scope

- Memory safety bugs (despite Rust, the `unsafe` blocks in `tray.rs`,
  `portcheck.rs`, and `sysstat.rs` are FFI surfaces that warrant attention).
- AMF0 / RTMP parsing edge cases that could crash the ingest task or
  corrupt the ring buffer.
- The web UI: the HTTP handler is hand-rolled. CSRF guard, body-size cap,
  Accept-Encoding handling, etc. Report anything unexpected.
- Stream-key leakage paths. Keys should never appear in logs, JSON
  responses (except `stream_key_set: bool`), or wire dumps.

## What's NOT in scope

- The listener binds **127.0.0.1** by default. Exposing it to a public
  network with `ingest_bind_all=true` / `web_bind_all=true` is explicitly
  an "I know what I'm doing" mode. Don't report "the public port is open"
  on a config you set to be public.
- Denial-of-service from a co-resident attacker (someone already running
  code on the same machine). That attacker has bigger problems available.
- Findings against an unmaintained fork.

## Supported versions

The latest released version on the `main` branch only. There are no LTS
branches; this is a hobby project.
