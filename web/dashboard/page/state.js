// - State ------------------------------─
let lastState = null;
// Last /config snapshot - cached so the capacity-math helpers can read
// buffer_mb without re-fetching. Updated by loadConfig() and by the
// settings save flow.
let lastConfig = null;
let platforms = [];
let twitchIngests = null;
let profiles = [];
let lastDests = [];
let pendingPhase = null;
let pendingClearT = null;
let destCardCache = {};
let rateHistory = [];
let cpuHistory = [];
const HISTORY_LEN = 60;
const PHASE_TO_STATE = { idle:'off', preparing:'arming', ready:'armed', active:'active' };
// State pill composes OBS-state + delay-state. "phase=idle" alone doesn't
// mean "off air" - it just means "no delay armed" (passthrough). Only flag
// OFFLINE when ingest is actually down.
function computeStatePill(s){
  if (!s || !s.ingest_alive){
    // Armed with nothing publishing is pre-arm, which is a real workflow -
    // but the buffer cannot fill until OBS starts, so saying "filling"
    // would be a progress bar that never moves.
    // Armed with nothing publishing can only mean OBS dropped after arming:
    // it cannot be entered from offline any more.
    const pre = s && (PHASE_TO_STATE[s.phase] === 'arming' || PHASE_TO_STATE[s.phase] === 'armed');
    if (pre) return { label:'OFFLINE', desc:'· still armed, waiting for OBS', c:'var(--warn)', pulse:false };
    // The line under the number says what it waits for: said once.
    return { label:'OFFLINE', desc:'', c:'var(--idle)', pulse:false };
  }
  const ds = PHASE_TO_STATE[s.phase] || 'off';
  if (ds === 'off')    return { label:'LIVE',      desc:'· no delay',        c:'var(--live)',   pulse:true };
  if (ds === 'arming') return { label:'ARMING',    desc:'· getting ready',   c:'var(--warn)',   pulse:false };
  if (ds === 'armed')  return { label:'ARMED',     desc:'· press Activate',  c:'var(--accent)', pulse:false };
  if (ds === 'active') return { label:'ACTIVE',    desc:'· Delay live',     c:'var(--live)',   pulse:true };
  return { label:'IDLE', desc:'', c:'var(--idle)', pulse:false };
}
const PLATFORM_INITIAL = { twitch:'T', youtube:'Y', kick:'K', trovo:'V', restream:'R', custom:'·', sink:'⌂' };
// Platform marks, drawn in currentColor so each surface tints them with the
// platform's colour. Twitch, YouTube and Kick are their own glyphs; the
// relays and local targets get a plain symbol rather than an imitation logo.
const PLATFORM_MARK = {
  twitch: '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M4.3 3 3 6.4V19h4.4v2.4h2.4L12.2 19h3.6l4.9-4.9V3zm14.8 10.4-2.8 2.8h-4.4l-2.4 2.4v-2.4H5.8V4.6h13.3zM15.2 7.3h1.6v4.8h-1.6zm-4.4 0h1.6v4.8h-1.6z"/></svg>',
  youtube: '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path fill-rule="evenodd" d="M21.6 7.2a2.5 2.5 0 0 0-1.8-1.8C18.2 5 12 5 12 5s-6.2 0-7.8.4A2.5 2.5 0 0 0 2.4 7.2 26 26 0 0 0 2 12c0 1.7.1 3.3.4 4.8a2.5 2.5 0 0 0 1.8 1.8c1.6.4 7.8.4 7.8.4s6.2 0 7.8-.4a2.5 2.5 0 0 0 1.8-1.8c.3-1.5.4-3.1.4-4.8s-.1-3.3-.4-4.8zM10 15.1V8.9l5.3 3.1z"/></svg>',
  kick: '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M4 3h5v5h2V6h2V4h2V3h5v6h-2v2h-2v2h2v2h2v6h-5v-1h-2v-2h-2v-2H9v5H4z"/></svg>',
  restream: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="5" cy="12" r="2"/><path d="M7 12h12M11 12l5-5h3M11 12l5 5h3"/></svg>',
  custom: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="4" y="4" width="16" height="6" rx="1.5"/><rect x="4" y="14" width="16" height="6" rx="1.5"/><path d="M8 7h.01M8 17h.01"/></svg>',
  sink: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
};
// The streaming apps' own marks (Simple Icons paths), shown where the
// dashboard names the app: the wizard's app pick and the OBS tab's cards.
const APP_MARK = {
  obs: '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M12,24C5.383,24,0,18.617,0,12S5.383,0,12,0s12,5.383,12,12S18.617,24,12,24z M12,1.109 C5.995,1.109,1.11,5.995,1.11,12C1.11,18.005,5.995,22.89,12,22.89S22.89,18.005,22.89,12C22.89,5.995,18.005,1.109,12,1.109z M6.182,5.99c0.352-1.698,1.503-3.229,3.05-3.996c-0.269,0.273-0.595,0.483-0.844,0.78c-1.02,1.1-1.48,2.692-1.199,4.156 c0.355,2.235,2.455,4.06,4.732,4.028c1.765,0.079,3.485-0.937,4.348-2.468c1.848,0.063,3.645,1.017,4.7,2.548 c0.54,0.799,0.962,1.736,0.991,2.711c-0.342-1.295-1.202-2.446-2.375-3.095c-1.135-0.639-2.529-0.802-3.772-0.425 c-1.56,0.448-2.849,1.723-3.293,3.293c-0.377,1.25-0.216,2.628,0.377,3.772c-0.825,1.429-2.315,2.449-3.932,2.756 c-1.244,0.261-2.551,0.059-3.709-0.464c1.036,0.302,2.161,0.355,3.191-0.011c1.381-0.457,2.522-1.567,3.024-2.935 c0.556-1.49,0.345-3.261-0.591-4.54c-0.7-1.007-1.803-1.717-3.002-1.969c-0.38-0.068-0.764-0.098-1.148-0.134 c-0.611-1.231-0.834-2.66-0.528-3.996L6.182,5.99z"/></svg>',
  streamlabs: '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8.6878 1.3459a1.365 1.365 0 0 0-.2734.0058c-.528.066-1.0133.1616-1.4843.3086A10.0568 10.0568 0 0 0 .3208 8.2697c-.147.471-.2445.9583-.3105 1.4863-.091.734.431 1.4041 1.166 1.4961.734.091 1.404-.43 1.496-1.164.05-.406.119-.7316.209-1.0196A7.3736 7.3736 0 0 1 7.727 4.221c.288-.09.6145-.157 1.0195-.207.735-.092 1.255-.7631 1.164-1.4981a1.3394 1.3394 0 0 0-1.2226-1.17Zm4.0488 5.2226c-2.629 0-3.9432.0007-4.9472.5117A4.684 4.684 0 0 0 5.7406 9.131c-.512 1.004-.5117 2.3183-.5117 4.9473v4.289c0 1.502-.001 2.2542.291 2.8282.257.505.6679.9149 1.1719 1.1719.574.292 1.326.291 2.828.291h6.9706c2.628 0 3.9442.0012 4.9472-.5098a4.6883 4.6883 0 0 0 2.0507-2.0508c.512-1.004.5117-2.3182.5117-4.9472v-1.0723c0-2.629.0003-3.9433-.5117-4.9473a4.6883 4.6883 0 0 0-2.0507-2.0508c-1.003-.511-2.3193-.5117-4.9472-.5117zm.537 6.7051c.741 0 1.3399.5998 1.3399 1.3398v2.6836c0 .74-.5988 1.3399-1.3398 1.3399-.74 0-1.3418-.5999-1.3418-1.3399v-2.6836c0-.74.6018-1.3398 1.3418-1.3398zm5.3632 0c.74 0 1.3399.5998 1.3399 1.3398v2.6836c0 .74-.5999 1.3399-1.3399 1.3399-.741 0-1.3398-.5999-1.3398-1.3399v-2.6836c0-.74.5989-1.3398 1.3398-1.3398z"/></svg>',
};
function fillAppMarks(){
  document.querySelectorAll('[data-app-mark]').forEach(el => { el.innerHTML = APP_MARK[el.dataset.appMark] || ''; });
}
// The mark for `slug`, or its letter when there's no mark (Trovo, unknown).
function platformMark(slug){
  if (PLATFORM_MARK[slug]) return PLATFORM_MARK[slug];
  const letter = document.createElement('span');
  letter.textContent = PLATFORM_INITIAL[slug] || (slug || '?').charAt(0).toUpperCase();
  return letter.outerHTML;
}
const PLATFORM_LABEL = { twitch:'Twitch', youtube:'YouTube', kick:'Kick', trovo:'Trovo', restream:'Restream', custom:'Custom', sink:'Test destination' };
// Fixed ports of the managed test-sink child - mirror config.rs
// SINK_RTMP_PORT / SINK_WEB_PORT (documented as non-configurable).
const SINK_PLAYER_URL = 'http://127.0.0.1:19351/';
const SETTINGS_FIELDS = {
  'ingest_port':'s-iport','web_port':'s-wport','buffer_mb':'s-bmb','buffer_path':'s-bpath',
  'overlays_dir':'s-ovdir',
};

// - Formatting ----------------------------─
// "45s", "5m", "1m 03s", "1h 30m": the way the app's own messages write a
// duration, so the dashboard and a refusal never spell one two ways.
const fmtDelay = ms => {
  const s = Math.round(ms/1000);
  if (s < 60) return s+'s';
  const m = Math.floor(s/60), r = s%60;
  if (m >= 60) return m % 60 ? `${Math.floor(m/60)}h ${m % 60}m` : `${m/60}h`;
  return r ? `${m}m ${String(r).padStart(2, '0')}s` : `${m}m`;
};
const fmtKbps = k => k >= 1000 ? (k/1000).toFixed(2)+' Mbps' : k+' kbps';
function fmtBytesPair(b){ const u=['B','KB','MB','GB','TB']; let i=0; while(b>=1024 && i<u.length-1){b/=1024;i++} return [b.toFixed(b<10&&i?1:0), u[i]]; }

// - Buffer capacity ----------------------------
// How long a delay the buffer holds comes from the server: the state's
// buffer_capacity_ms_est (Controller::delay_capacity_ms), worked out from
// the real buffer file and the bitrate OBS sends, held at its recent peak
// so a dip doesn't unlock a delay the buffer can't hold. The dashboard
// gates on that same number, so it never disagrees with what /arm accepts.
// The helpers below are the same maths for a size or a delay that isn't in
// use: the System tab's buffer field and the "needs at least N MB" advice.
const BUF_REF_KBPS = 10000;  // what the server plans for before a stream is measured
const BUF_SLACK_MS = 2000;   // kept on top of the delay (BUFFER_SLACK_MS)
function planningKbps(){ return (lastState && lastState.capacity_kbps) || BUF_REF_KBPS; }
const capacityRate = () => `${(planningKbps()/1000).toFixed(1)} Mbps`;
// The longest delay a buffer of `bufferMB` holds at `kbps`: all of the file
// but a sixteenth fills (DiskRing::usable_bytes), less the slack.
function bufferCapMs(bufferMB, kbps){
  if (!bufferMB || bufferMB <= 0) return 0;
  const bytes = bufferMB * 1048576;
  const usable = bytes - Math.floor(bytes / 16);
  return Math.max(0, Math.floor(usable * 8 / kbps) - BUF_SLACK_MS);
}
// The buffer size, in MB, that holds a delay of `ms` at the planning
// bitrate, rounded up like the server's (buffer_mb_for).
function requiredMBForMs(ms){
  if (!ms || ms <= 0) return 0;
  const bytes = Math.ceil((ms + BUF_SLACK_MS) * planningKbps() / 8);
  return Math.ceil(Math.ceil(bytes * 16 / 15) / 1048576);
}
// What the buffer holds right now, from the server. 0 before the first state.
function currentBufferCapMs(){ return (lastState && lastState.buffer_capacity_ms_est) || 0; }
// The buffer in use, which is what the capacity comes from. A bigger size
// saved in System only counts after a restart (restart_pending).
function runningBufferMb(){
  return (lastState && lastState.buffer_mb_running) || (lastConfig && lastConfig.buffer_mb) || '?';
}
// The "buffer too small" advice: what to do about it, with a Show me that
// fills the size in on System without saving. A saved size that would hold
// it but isn't running yet asks for the restart instead.
function bufferAdviceHtml(ms){
  const saved = lastConfig ? lastConfig.buffer_mb : 0;
  if (lastState && lastState.restart_pending && bufferCapMs(saved, planningKbps()) >= ms){
    return ` Your new ${saved} MB buffer is saved and starts with the next restart of InstantClone. <button type="button" class="cap-show" data-mb="0">Show me</button>`;
  }
  return ` Make the buffer bigger in System (it needs a restart). <button type="button" class="cap-show" data-mb="${requiredMBForMs(ms)}">Show me</button>`;
}
// Live hint under the System-tab buffer-size input. Reads whatever
// number the user has *currently typed* (not the saved value) so the
// "X MB ≈ Ys max delay" feedback updates as they're picking a size.
// When a stream is live, we also surface the same math at the
// measured bitrate so they can see how much disk it actually buys
// them right now.
function renderBufferHint(){
  const el = $('s-bmb-hint');
  if (!el) return;
  const mb = parseInt($('s-bmb').value || '0', 10) || 0;
  if (mb <= 0){ el.textContent = ''; return; }
  // Past the longest delay there is, more buffer buys nothing.
  const top = ms => fmtDelay(Math.min(ms, DELAY_MAX_S * 1000));
  const reference = `${mb} MB → max ${top(bufferCapMs(mb, BUF_REF_KBPS))} delay at ${BUF_REF_KBPS/1000} Mbps reference.`;
  // Two sentences, two text nodes: the translator looks each up on its own.
  el.replaceChildren(reference);
  if (lastState && lastState.ingest_alive){
    el.append(` Currently ≈ ${top(bufferCapMs(mb, planningKbps()))} at ${capacityRate()} in.`);
  }
}

