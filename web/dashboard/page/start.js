// - Start: runs last, once every piece above has set itself up --------------
(async function init(){
  // The counters before anything is fetched: a change landing while the page
  // loads moves one past this, and the first state frame fetches it again.
  try { Live.revs = (await (await fetch('/state')).json()).rev || null; } catch(_){}
  try { platforms = await (await fetch('/platforms')).json(); } catch(_){ platforms = []; }
  fillWizardPlatforms();
  wireAllDisclosures();
  reserveCopyButtons();
  wireChatDock();
  fillAppMarks();
  await loadConfig();
  langReturnReady();
  refreshObsRegisterStatus();
  refreshStreamlabs();
  Live.on('config', () => loadConfig({live: true}));
  Live.on('destinations', loadDestinations);
  Live.on('profiles', loadProfiles);
  Live.on('overlays', refreshOverlays);
  Live.on('obs', refreshObsSetup);
  Live.on('streamlabs', refreshStreamlabs);
  checkForUpdate();
  loadWhatsNew();
  checkUpdateOutcome();
  startStateSSE();
  // A busy log moves its counter many times a second: read it once a second.
  Live.on('logs', throttled(pollLogs, 1000));
  pollLogs();
  document.addEventListener('visibilitychange', () => { if (!document.hidden) pollLogs(); });
  // Space drives the delay only from the bare page: a focused control
  // (button, switch, select, style card) or an open dialog keeps it, and
  // holding the key down doesn't repeat the action.
  document.addEventListener('keydown', e => {
    if (e.key !== ' ' || e.repeat) return;
    if (e.target.closest('input,textarea,select,button,a,[role],[tabindex],[contenteditable]')) return;
    if (document.querySelector('.ic-modal:not([hidden]),#ov-studio:not([hidden]),#tour:not([hidden])')) return;
    e.preventDefault();
    ctaClick({currentTarget: $('cp-cta')});
  });
  // First-run tour autostart lives inside loadConfig() (see the
  // `if (configured)` branch). That hook fires both on the
  // wizard-to-dashboard transition AND on initial load when the user is
  // already configured, with the localStorage flag preventing double-
  // fires. Doing it here would race the wizard (hdr-bar hidden → skip)
  // and leave first-run users without a tour after they save settings.
})();
