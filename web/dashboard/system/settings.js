// - Config / Settings ------------------------─
let dashboardShown = false;
// `resetSystem`: overwrite the System form even with unsaved edits (after a
// System save, or Discard). Any other reload leaves those edits alone.
// The tab icon in this copy's tray colour, so with two InstantClones
// running, each dashboard tab matches its own tray icon.
function paintFavicon(color){
  if (!/^#[0-9a-f]{6}$/i.test(color || '')) return;
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 48 48'><rect x='3' y='3' width='42' height='42' rx='10.5' fill='${color}'/><path d='M17 14h4v20h-4zM27 14h4v20h-4z' fill='#f4f6f9'/></svg>`;
  // A new <link>, not a new href on the old one: Chrome keeps the tab's
  // icon when only the href changes.
  const link = document.createElement('link');
  link.rel = 'icon';
  link.id = 'favicon';
  link.href = 'data:image/svg+xml,' + encodeURIComponent(svg);
  $('favicon').replaceWith(link);
}
// The language changed in another window: this one follows, unless it holds
// settings not saved yet. The wizard's own pick reloads by itself (welPick).
let languageToFollow = false;
function followLanguage(was, c){
  if (was === undefined) return;
  if (was !== c.ui_language) languageToFollow = true;
  if (!languageToFollow || !$('wizard').hidden) return;
  if (_savingSettings || settingsDirty()){
    // Saving those settings must not put the old language back.
    $('s-language').value = c.ui_language || 'auto';
    return;
  }
  languageToFollow = false;
  if (languageOf(c.ui_language || 'auto') !== document.documentElement.lang) reloadForLanguage();
}
async function loadConfig({resetSystem = false, live = false} = {}){
  try {
    const c = await (await fetch('/config')).json();
    const languageWas = lastConfig ? lastConfig.ui_language : undefined;
    lastConfig = c;
    followLanguage(languageWas, c);
    // First run: install the built-in presets as real overlays (once).
    // NOT gated on `configured` - building overlays doesn't need a streaming
    // destination, so someone who clicks "Not now - let me look around" gets
    // the full gallery too. seedPresets is a no-op once the flag is set.
    if (!c.overlays_seeded && window.ICOverlay) { Studio.seedPresets(); }
    // The wizard starts from the saved setup when it opens. Seeded on every
    // reload, it would undo the picks made in it whenever a change arrived
    // from elsewhere (see Live).
    const wizardOpening = !c.configured && !wizardSkipped && $('wizard').hidden;
    if (wizardOpening){
      const first = (c.destinations && c.destinations[0]) || {platform:'twitch', custom_egress_url:'', stream_key_set:false};
      if ($('w-platform').children.length === 0) fillWizardPlatforms();
      $('w-platform').value = first.platform; $('w-platform').dispatchEvent(new Event('change'));
      $('w-custom').value = first.custom_egress_url || '';
    }
    // system fields
    if (dataChanged('favicon', c.tray_color)) paintFavicon(c.tray_color);
    $('ov-dir-display').textContent = c.overlays_dir || 'overlays/';
    applyPlatformCopy(c.os);
    if (resetSystem || !settingsDirty()){
      $('s-iport').value = c.ingest_port; $('s-wport').value = c.web_port;
      $('s-bmb').value = c.buffer_mb; $('s-bpath').value = c.buffer_path;
      renderBufferHint();
      $('s-ibind').checked = c.ingest_bind_all; $('s-wbind').checked = c.web_bind_all;
      renderLanWarning();
      $('s-ovdir').value = c.overlays_dir || '';
      $('s-trace').checked = c.tracing_enabled !== false;
      $('s-twitch-client').value = c.twitch_client_id || '';
      renderTwitchApp(c);
      // Reflects the actual Run-key entry, not a stored preference - so if
      // the user removed it via Task Manager's Startup tab, this shows off.
      $('s-startup').checked = !!c.start_with_windows;
      // Both default true server-side; treat a missing field as on.
      $('s-open-dash').checked = c.open_dashboard_on_launch !== false;
      $('s-language').value = c.ui_language || 'auto';
      $('s-update-check').checked = c.update_check_enabled !== false;
      $('s-update-tray').checked = c.update_tray_notify !== false;
      syncUpdateTray(false);
      if (resetSystem || dataChanged('tray-colors', [c.os, c.tray_colors, c.tray_color_choice, c.tray_color])) renderTrayColors(c);
      // Behavior section
      $('s-auto-arm').checked = !!c.auto_arm_on_connect;
      $('s-auto-act').checked = !!c.auto_activate_when_ready;
      $('s-auto-arm-secs').value = Math.round((c.auto_arm_delay_ms || 15000) / 1000);
      // Its preview is redrawn by the server: only when it changed.
      if (resetSystem || dataChanged('crash-protection', c.crash_protection)) cpApplyConfig(c.crash_protection);
      settingsMarkClean();
    }
    // Global hotkeys: Windows-only, so the whole section stays hidden
    // elsewhere. A pending recording is cancelled before a re-render so
    // stale listeners never outlive the DOM they were bound to; a reload
    // that brings the same bindings leaves a recording alone.
    if ($('s-hotkeys-section')){
      $('s-hotkeys-section').hidden = c.os !== 'windows';
      if (c.os === 'windows' && dataChanged('hotkeys', c.hotkeys)){
        if (hkRec) hkCancelRecording();
        hkBindings = Object.assign(emptyBindings(), c.hotkeys || {});
        hkRenderList();
      }
    }
    // MIDI mapping: Windows-only, same gating as the hotkeys. A one-off
    // poll fills in device availability; learn mode is driven separately.
    if ($('s-midi-section')){
      $('s-midi-section').hidden = c.os !== 'windows';
      if (c.os === 'windows' && dataChanged('midi', [c.midi, c.midi_device])){
        if (midiLearnAction) midiCancelLearn();
        midiBindings = Object.assign(emptyBindings(), c.midi || {});
        midiSelected = c.midi_device || '';
        midiInit();
      }
    }
    syncControlsTab();
    // OBS panel
    $('obs-srv').textContent = c.obs_url;
    if ($('wiz-obs-srv')) $('wiz-obs-srv').textContent = c.obs_url;
    if (dataChanged('ingest-key', c.ingest_key)) applyIngestKey(c.ingest_key || '');
    // Redrawn, the dock link would hide again while it's being read.
    if (dataChanged('auth', [c.auth_enabled, c.dock_token])) applyAuthState(!!c.auth_enabled, c.dock_token || '');
    if ($('sys-os-chip')){
      const osLabel = {windows:'Windows', linux:'Linux', macos:'macOS'}[c.os] || (c.os || '');
      if (osLabel){ $('sys-os-chip').textContent = osLabel; $('sys-os-chip').hidden = false; }
    }
    // The version row shows the running version from /update-check; seed it from /config so
    // it's correct immediately even before the (async) update check returns.
    if ($('sys-about-version') && c.version && $('sys-about-version').textContent === 'v?.?.?'){
      $('sys-about-version').textContent = 'v' + c.version;
    }
    // System-info tiles: static bits from /config (live bits filled per tick).
    // Show the port + exposure scope, never an IP - no address should be
    // readable off a screen-share or stream.
    const scope = b => b ? 'network' : 'local';
    if ($('si-ingest')) $('si-ingest').textContent = c.ingest_port + ' · ' + scope(c.ingest_bind_all);
    if ($('si-web')) $('si-web').textContent = c.web_port + ' · ' + scope(c.web_bind_all);
    if ($('si-buffer')) $('si-buffer').textContent = (c.buffer_mb || 0) + ' MB';
    // footer
    $('foot-rtmp').textContent = ':' + c.ingest_port;
    $('foot-web').textContent = ':' + c.web_port;
    // visibility. "Skip setup" shows the dashboard for this visit with
    // nothing saved, so `configured` alone would bring the wizard back.
    const configured = c.configured;
    // A save inside the wizard can set `configured` (a key imported from
    // Streamlabs): only finishing it closes it, not the reload that follows.
    const wizardStays = live && !$('wizard').hidden;
    const dashboard = (configured || wizardSkipped) && !wizardStays;
    const appearing = dashboard && $('pane-card').hidden;
    $('wizard').hidden = dashboard;
    if (wizardOpening) { refreshWizObsStatus(); restoreWizDraft(); wizCurrent = 'welcome'; wizDir = 'fwd'; renderWizStep(); }
    $('hdr-bar').hidden = !dashboard;
    $('hero').hidden = !dashboard;
    $('tabs').hidden = !dashboard;
    $('pane-card').hidden = !dashboard;
    $('ic-foot').hidden = !dashboard; fitPaneCard();
    if (configured){
      // Fetched when the dashboard appears; later changes come through Live.
      if (appearing){ loadDestinations(); loadProfiles(); loadOverlays(); }
      // Show a tab the first time the dashboard appears. Later reloads keep
      // the view as it is rather than replaying its entrance.
      if (!dashboardShown){
        dashboardShown = true;
        const cur = document.querySelector('.tab.on') || document.querySelector('.tab[data-tab="destinations"]');
        if (cur) showTab(cur.dataset.tab);
      }
      setTimeout(moveTabInd, 50);
      // First-run tour: fires on the first loadConfig() that sees
      // configured=true, which is the right moment for both fresh users
      // (just finished the wizard) and returning users (already
      // configured on initial load). openTour() sets the localStorage
      // flag so this no-ops on all subsequent loadConfig calls.
      try {
        if (!localStorage.getItem('instantclone_onboarded')){
          // Slightly longer delay than the initial-load path: the
          // wizard-to-dashboard transition has more layout to settle
          // (tabs, hero, footer all un-hiding at once).
          setTimeout(() => openTour(0), 900);
        }
      } catch(_){}
    }
  } catch(e){ console.warn(e); }
}

// Twitch mobile-decoder warning. Twitch's transcoded quality ladder
// (1080p / 720p / 480p / 360p / 160p) is account-tier gated, NOT
// bitrate gated - non-Affiliate accounts get Source-Only regardless
// of bitrate, Affiliate / Partner get the ladder. When the stream
// IS in Source-Only mode (no ladder), every viewer has to decode
// the full source bitrate. At ≤ 8 Mbps 1080p60 H.264 most mobile
// hardware decoders cope; above ~8 Mbps they start failing - that's
// the original beta.4 symptom (Error #1000 on PC, black screen with
// audio on mobile). The 8000 cap matches what the user verified is
// the "sweet spot" on an actual no-Affiliate alt: 8k Source-Only is
// watchable, 10k Source-Only is not.
//
// We can't tell from the wire whether the user's account has the
// transcoded ladder, so we warn unconditionally on Twitch + > 8 Mbps.
// Streamers with confirmed transcoded ladders can dismiss it; the
// alternative (silently letting beta.4 happen again) is worse.
//
// The check reads the video bitrate alone (viewers decode video; the
// audio and VOD tracks don't load the decoder), averaged over 10 s. The
// meter samples 1 s windows, and with a 2 s keyframe interval every other
// window holds a keyframe, so a steady 8 Mbps stream reads 7.4, 9.1,
// 7.6... and a raw threshold made the chip flicker. Showing above 8.0 and
// clearing below 7.7 keeps it steady when the average sits on the line.
const TWITCH_MOBILE_SAFE_KBPS = 8000;
const TWITCH_MOBILE_CLEAR_KBPS = 7700;
const TWITCH_CAP_AVG_MS = 10000;
let twitchCapSamples = [];
let twitchCapOn = false;
let twitchCapKbps = 0;
// Average of the last TWITCH_CAP_AVG_MS of samples, or 0 until most of a
// window has been seen, so a stream's first second can't light the chip.
function smoothedVideoKbps(s){
  const kbps = (s.stats && s.stats.video_kbps) || 0;
  if (!s.ingest_alive || kbps <= 0){ twitchCapSamples = []; return 0; }
  const now = Date.now();
  twitchCapSamples.push({t: now, kbps});
  while (now - twitchCapSamples[0].t > TWITCH_CAP_AVG_MS) twitchCapSamples.shift();
  if (now - twitchCapSamples[0].t < TWITCH_CAP_AVG_MS * 0.8) return 0;
  return twitchCapSamples.reduce((sum, x) => sum + x.kbps, 0) / twitchCapSamples.length;
}
function renderTwitchCapPill(s){
  const pill = document.getElementById('hdr-twitch-cap');
  const v = document.getElementById('hdr-twitch-cap-v');
  if (!pill || !v) return;
  // /state's destination entries carry no platform, so join on id.
  const platformOf = id => ((lastDests || []).find(x => x.id === id) || {}).platform;
  const hasTwitchDest = (s.destinations || []).some(
    d => platformOf(d.id) === 'twitch' && d.alive && d.enabled
  );
  const kbps = smoothedVideoKbps(s);
  twitchCapOn = twitchCapOn ? kbps >= TWITCH_MOBILE_CLEAR_KBPS : kbps > TWITCH_MOBILE_SAFE_KBPS;
  twitchCapKbps = kbps;
  // When Enhanced Broadcasting is active the multi-track stream
  // bypasses Twitch's account-tier source-only fallback - viewers
  // get the full transcoded ladder regardless of bitrate, so the
  // mobile-decoder warning we'd otherwise show is moot. The "⚡
  // Enhanced Broadcasting" chip next to this one already
  // communicates the EB-active state, so silently suppress this
  // warning to avoid two chips telling the user different things.
  const ebActive = !!s.multitrack_video;
  if (hasTwitchDest && twitchCapOn && !ebActive){
    v.textContent = `Twitch · mobile risk at ${(kbps/1000).toFixed(1)} Mbps`;
    pill.hidden = false;
    pill.style.cursor = 'pointer';
    pill.title = '';
    pill.onclick = openTwitchCapInfo;
  } else {
    pill.hidden = true;
    pill.onclick = null;
  }
}
function openTwitchCapInfo(){
  const at = twitchCapKbps ? `${(twitchCapKbps/1000).toFixed(1)} Mbps` : 'your current bitrate';
  toast(
    `If Twitch shows your stream as "Source-Only" (no quality ladder), viewers must decode the full ${at}. Above ~8 Mbps that's too heavy for many mobile decoders: they hit Error #1000 or black screen with audio. The transcoded ladder is account-tier gated (Affiliate / Partner), not bitrate gated. If you're not sure your account gets transcoded, drop OBS to ≤ 8 Mbps for safe mobile playback. If you know it's transcoded, ignore this - viewers will pick lower qualities themselves.`,
    'err'
  );
}

// Chat-dock URL builder for the OBS tab. The popout/embed URL pattern
// is well-defined for each platform (and stable; these endpoints have
// been the same for years), so we build them client-side rather than
// adding another /api round-trip just to template a string. The user
// pastes the result into OBS as a Custom Browser Dock - no server
// state is touched.
//
// YouTube is the odd one out: live chat URLs key off the per-broadcast
// video ID, not the channel name. We let the user paste either the
// video ID or the full Studio/watch URL and extract the id with a
// short regex.
const CHAT_DOCK_URLS = {
  twitch:  u => `https://www.twitch.tv/popout/${u}/chat?popout=`,
  kick:    u => `https://kick.com/popout/${u}/chat`,
  trovo:   u => `https://trovo.live/chat/${u}`,
};
function wireChatDock(){
  const sel = document.getElementById('chat-dock-platform');
  const inp = document.getElementById('chat-dock-user');
  const out = document.getElementById('chat-dock-url');
  if (!sel || !inp || !out) return;
  const update = () => {
    const raw = inp.value.trim();
    if (sel.value === 'youtube'){
      inp.placeholder = 'video ID or watch URL of your live broadcast';
      if (!raw){ out.textContent = 'paste your live broadcast video ID above'; return; }
      const m = raw.match(/[?&]v=([\w-]{11})|youtu\.be\/([\w-]{11})|^([\w-]{11})$/);
      const id = m ? (m[1] || m[2] || m[3]) : null;
      out.textContent = id
        ? `https://www.youtube.com/live_chat?is_popout=1&v=${id}`
        : `couldn't read a YouTube video ID from "${raw}"`;
      return;
    }
    inp.placeholder = 'your channel name';
    if (!raw){ out.textContent = 'enter your channel name above'; return; }
    const fn = CHAT_DOCK_URLS[sel.value];
    out.textContent = fn ? fn(raw) : raw;
  };
  sel.addEventListener('change', update);
  inp.addEventListener('input', update);
}

// Per-platform accent, mirroring the .dcard.platform-* CSS so a wizard
// tile matches the destination card the same choice produces.
const PLATFORM_COLOR = { twitch:'#a78bfa', youtube:'#fda4af', kick:'#86efac',
  trovo:'#67e8f9', restream:'#5eead4', custom:'#fcd34d', sink:'#93c5fd' };

