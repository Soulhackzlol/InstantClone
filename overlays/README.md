# InstantClone overlays

On-stream browser-source widgets that show your delay state.

Most people never touch this folder. The dashboard's **Overlay** tab is a
no-code **Studio**: it ships with ready-made overlays you copy a URL from
and drop into OBS, and you can open any of them to redesign (per-state
colours, widgets, animations) and **Save** or **Save as new**. The
built-in overlays are baked to real files in this folder on first run, so
everything you see in the tab is a plain `.html` with its own URL:

    http://127.0.0.1:7799/overlay/your-file.html

You can still hand-write one: any `.html` you drop in here shows up in the
Overlay tab as a "legacy" (hand-written) overlay. The only contract is the
URL above and the `/state` JSON below.

## What the proxy will tell you

Every overlay can fetch live state from `GET /state` (or subscribe to the
`GET /events` SSE stream, which pushes the same JSON on every change):

    {
      "phase": "idle" | "preparing" | "ready" | "active",
      "armed_delay_ms":   <u32>,   // what the user set
      "current_delay_ms": <u32>,   // what's actually being delivered
      "target_delay_ms":  <u32>,   // post-cut target
      "buffer_fill_ms":   <u32>,   // 0..armed during preparing
      "ingest_alive":     <bool>,  // OBS connected to us
      "egress_alive":     <bool>,  // at least one destination forwarding
      "destinations_alive": <u32>,
      "destinations_total": <u32>,
      "stats": { "tags_sent", "bytes_sent", "cuts", "bitrate_kbps" }
    }

Typical poll loop:

    <script>
      async function tick(){
        try {
          const s = await (await fetch('/state')).json();
          document.getElementById('v').textContent =
            (s.current_delay_ms / 1000).toFixed(1);
        } catch (_) {}
      }
      tick(); setInterval(tick, 500);
    </script>

## Writing your own

- **Easiest:** the Studio. Pick an overlay in the Overlay tab, **Edit in
  Studio**, then **Save** (overwrite) or **Save as new**.
- **By hand:** copy **`custom-template.html`** - a small working overlay
  with comments explaining every part of the contract above. Rename the
  copy to whatever you want; it appears in the Overlay tab.
- **No file at all:** the legacy quick styles still work straight from a
  URL - `/overlay?style=minimal|corner|strip|focus|broadcast|ticker` with
  an optional `?lang=en|es|pt|fr|de`. They live in
  [`src/web.rs`](../src/web.rs)'s `overlay_html()`; open one in DevTools to
  crib the auto-dim, number-tween, or phase-halo behaviour.

> The Studio's built-in overlays are seeded once and then yours to edit or
> delete; **Restore default overlays** (in the Overlay tab) reinstalls
> them. Seeded/Studio overlays are local, per-install files and aren't
> tracked in git - only the hand-written ones here are.
