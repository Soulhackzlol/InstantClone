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
