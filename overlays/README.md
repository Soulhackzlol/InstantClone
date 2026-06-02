# InstantClone overlay plugins

Drop any `.html` file into this folder and it becomes an OBS browser-source
overlay. The dashboard's *Overlay* tab lists everything in here next to the
six built-in styles, and gives you the URL to paste into OBS:

    http://127.0.0.1:7799/overlay/your-file.html?lang=en

## What the proxy will tell you

Every overlay can fetch live state from `GET /state`:

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
      "bitrate_kbps":     <u32>,   // ingest bitrate
      "stats": { "tags_sent", "bytes_sent", "cuts" }
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

## Starting points

- **`custom-template.html`** - copy this. It's a small working overlay
  with comments explaining every part of the contract above. Rename the
  copy to whatever you want; it'll appear in the dashboard.
- **The six built-in styles** (`minimal`, `corner`, `strip`, `focus`,
  `broadcast`, `ticker`) are served from `/overlay?style=…` and live in
  [`src/web.rs`](../src/web.rs)'s `overlay_html()`. Open one in DevTools
  if you want to crib the auto-dim, number-tween, or phase-halo
  behaviour into your own design.

The only contract is the URL above and the `/state` JSON.
