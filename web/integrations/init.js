window.Integrations = {
  async show(){
    S.visible = true;
    wireRoot();
    const root = $('ig-root');
    // Known data paints at once and refreshes quietly; a first visit shows
    // the skeleton until the app answers.
    if (S.data){
      render();
      enterStagger(root);
      load();
    } else if (root){
      root.innerHTML = skeletonHtml();
      if (await load() && S.visible) enterStagger(root);
    }
    if (S.visible) schedulePoll();
  },
  hide(){
    S.visible = false;
    if (!modalOpen() && !S.stage) clearInterval(S.pollTimer);
  },
  // The Overlay tab showing its Alerts or its Goals page (`sub`), or
  // neither any more.
  async stage(sub){
    S.stage = sub === 'alerts' || sub === 'goals' ? sub : null;
    if (!S.stage){
      clearTimeout(Stage.clockT);
      if (!S.visible && !modalOpen()) clearInterval(S.pollTimer);
      return;
    }
    wireStage();
    // Which goals have an overlay is the gallery's to say: ask each visit.
    if (S.stage === 'goals') Stage.loadSlugs().then(() => { if (S.stage === 'goals') Stage.render(); });
    if (S.data) Stage.render();
    else if (!(await load())){
      const root = $(S.stage === 'alerts' ? 'ov-alerts' : 'ov-goals');
      if (root) root.innerHTML = '<div class="lan-warn"><span class="lw-ic">⚠</span><span>The app did not answer. Is InstantClone still running?</span></div>';
    }
    // (Once the data is in: the default design is part of the alerts' look,
    // and the gallery says which alerts play each design.)
    if (S.stage === 'alerts' && S.data) Stage.loadDesigns().then(() => {
      if (S.stage !== 'alerts') return;
      Stage.render();
      if (typeof Studio !== 'undefined') Studio.alertsChanged();
    });
    if (S.stage) schedulePoll();
  },
  // A new goal or timer, from the Overlay tab's own button.
  async newTracker(what){
    if (!S.data && !(await load())) return;
    Stage.edit(what, '');
  },
  // For the Studio's pickers: your goals and timers, read once if need be.
  async tracking(){
    if (!S.data) await load();
    return tracking();
  },
  addOverlayToObs,
  // The Studio's Alert card layer: the themes, fonts and entrances it has.
  alertKit:{themes:THEMES, fonts:ALERT_FONTS, anims:ALERT_ANIMS, sounds:ALERT_SOUNDS},
  // The Overlay tab's Alerts page, the Studio's gallery of alert designs:
  // which alerts play one, one for every alert, a ready-made one made a
  // file, a deleted one let go, and a design's icon.
  designUse:slug => Stage.designUse(slug),
  useDesignEverywhere:slug => Stage.useEverywhere(slug),
  ensureDesign:slug => Stage.ensureDesign(slug),
  designGone:slug => Stage.designGone(slug),
  designIcon:designIconHtml,
  // The Studio's sound for a design: your uploads, one more, heard first.
  soundUploads:() => S.data ? mediaOf('sound').map(m => ['media:' + m.name, m.name]) : [],
  uploadSound:() => uploadMedia('sound'),
  playSound:sound => playAlertSound({title:'', text:'', sound, volume:'70'}),
  // The Alerts page with no alerts yet: Twitch alerts, opened to look at.
  async addTwitchAlerts(){
    if (!S.data && !(await load())) return;
    showTab('integrations');
    await addFrom({preset:'twitch_alerts'}, true);
  },
};

// Integrations changed elsewhere (another window, a Discord channel or a
// phone just connected): what this page already loaded follows, the tab
// and an open catalog, Connections page or Overlay page with it (see Live
// in the dashboard). An editor keeps its own copy until it's saved.
if (typeof Live === 'object') Live.on('integrations', async () => {
  if (!S.data) return;
  await refreshData();
  if (Modal.kind === 'catalog') Catalog.render();
  else if (Modal.kind === 'connections') Conn.render();
  if (S.stage) Stage.render();
});

// A reload or a restored tab can land straight on Integrations.
const pane = document.querySelector('.tab-pane[data-pane="integrations"]');
if (pane && !pane.hidden) window.Integrations.show();
// Or on the Overlay tab, with one of this file's pages open.
const overlayPane = document.querySelector('.tab-pane[data-pane="overlay"]');
if (overlayPane && !overlayPane.hidden){
  const open = overlayPane.querySelector('.sub-tab.on');
  window.Integrations.stage(open ? open.dataset.subtab : null);
}
})();
