# InstantClone overlay plugins

Any `.html` file you drop into this folder becomes an OBS browser-source
overlay. The dashboard's *Overlay* tab lists them and gives you the URL
to paste into OBS.

Each overlay can fetch live state from the proxy:

    GET /state  →  JSON  {
      phase: "idle"|"preparing"|"ready"|"active",
      current_delay_ms, armed_delay_ms, buffer_fill_ms, ...
      ingest_alive, egress_alive,
      destinations_alive, destinations_total,
      destinations: [ { id, name, enabled, alive, bitrate_kbps, ... } ],
      stats: { bitrate_kbps, cuts, micro_drops, ... }
    }

Recommended pattern:

    <script>
      async function tick(){
        try { const s = await (await fetch('/state')).json();
              document.getElementById('v').textContent = (s.current_delay_ms/1000).toFixed(1);
            } catch(_) {}
      }
      tick(); setInterval(tick, 500);
    </script>

The three bundled overlays (minimal, corner, strip) are useful starting
points — copy one and modify.
