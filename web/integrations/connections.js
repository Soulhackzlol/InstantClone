// ---------------------------------------------------------------- connections

// Where integrations post and listen: one card each, with how it stands.
// [id, icon, name, what it's for].
const CONNECTIONS = [
  ['discord', 'bubble', 'Discord', 'Messages and live-updating cards in your server.'],
  ['twitch', 'tv', 'Twitch', 'Chat, VOD markers, clips, and follows, subs and raids.'],
  ['phone', 'phone', 'Phone', 'Pushes to your phone, through the free ntfy app.'],
  ['obs', 'rec', 'OBS', 'Scenes, sources and recording, through OBS\'s WebSocket.'],
  ['stream', 'screen', 'Alerts page', 'Where alerts show up on stream, and how they look.'],
];

// How a connection stands: [pill class, words]. The strip and the cards
// say the same. The class is 'ok', 'warn', 'todo' (not set up yet: drawn
// as an invitation, not as switched off) or '' (on its way).
function connState(id){
  const c = S.data.connections, t = S.data.twitch;
  switch (id){
    case 'discord': return c.discord.length ? ['ok', plural(c.discord.length, 'channel')] : ['todo', 'Add'];
    case 'twitch':
      if (!t.main.login) return ['todo', t.available ? 'Connect' : 'Set up'];
      return t.main.chat === 'connected' ? ['ok', 'connected'] : ['warn', 'chat ' + t.main.chat];
    case 'phone': return c.phone.topic ? ['ok', 'connected'] : ['todo', 'Connect'];
    // The open dashboard keeps OBS connected. Out of reach only warns when
    // an integration needs it: otherwise it just has to be set up.
    case 'obs': {
      const o = S.data.obs || {};
      if (o.state === 'connected') return ['ok', 'connected'];
      if (o.state === 'error') return o.used ? ['warn', 'can\'t reach'] : ['todo', 'Set up'];
      return ['', 'connecting…'];
    }
    case 'stream': return alertsPill();
  }
  return ['', ''];
}

const Conn = {
  tab:'discord', editing:null,
  // `tab`: the card to open (none: all closed).
  open(tab){
    // Opened from an editor or an import: go back to it, work intact.
    const from = {editor:Editor, builder:Builder, import:Recipes}[Modal.kind] || null;
    this.tab = tab || null;
    this.editing = null;
    Modal.open('connections', 760);
    Modal.onClose = () => { if (S.data.twitch.login) api('/twitch/cancel', {}); return true; };
    if (from) Modal.after = () => from.resume();
    this.render();
  },
  resume(){ this.open(this.tab); },
  render(){
    // A slow reply can land after the user left this page.
    if (Modal.kind !== 'connections') return;
    const panes = {discord:() => this.discordHtml(), twitch:() => this.twitchHtml(), phone:() => this.phoneHtml(),
      obs:() => this.obsHtml(), stream:() => this.alertsHtml()};
    const cards = CONNECTIONS.map(([id, icon, name, blurb]) => {
      const open = this.tab === id, [cls, words] = connState(id);
      return `<section class="ig-cx${open ? ' open' : ''}" data-key="cx-${id}">
        <button class="ig-cx-head" data-act="conn-tab" data-tab="${id}" aria-expanded="${open}">
          <span class="dcard-icon ig-ic-${id}">${svg(icon)}</span>
          <span class="ig-cx-id"><b>${name}</b><span>${blurb}</span></span>
          <span class="ic-pill ${cls}"><span class="dot"></span><span class="ic-pill-v">${esc(words)}</span></span>
          <span class="ig-cx-chev">${svg('chev')}</span>
        </button>
        <div class="ig-cx-body" data-flip data-autoh>${open ? `<div class="ig-cx-in" data-key="cxb-${id}">${panes[id]()}</div>` : ''}</div>
      </section>`;
    }).join('');
    Modal.body(`${modalHead('link', 'var(--accent)', 'Connections', 'Where integrations post and what they listen to. Set each one up once.')}
      <div class="dest-form-body ig-conn-body"><div class="ig-cxs">${cards}</div></div>`);
    if (this.tab === 'obs') ObsLive.watch();
  },
  // The alerts page: in OBS or not, the design alerts play by default, and
  // the way to the Alerts page, where every alert's look is.
  alertsHtml(){
    const a = S.data.alerts || {}, look = lookOf();
    const url = a.url || `${location.origin}/alerts`;
    return `<div class="sys-section ig-form ig-setup">${alertsStatusHtml(false)}
      <div class="ig-look-sum"><span class="ig-look-dot" style="--sw:${alertColor('', look)}"></span>
        <span class="ig-look-sum-t">Alerts play <b>${esc(designLabel(''))}</b> unless they have a design of their own</span>
        <button class="ic-btn ic-btn-primary ig-small" data-act="alerts-open">Change the look</button></div>
      <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="alert-test">${svg('play', ' class="ig-btn-ic"')}Show a test alert</button>
      <details class="ig-more-fields"><summary>Add it to OBS by hand</summary>
        <div class="ig-insp-p">In OBS: <b>+</b> under Sources › <b>Browser</b>, paste this address, and make it as big as your canvas. Tick <b>Control audio via OBS</b> so alert sounds reach the stream.</div>
        <div class="ig-copy-row"><input class="ic-input mono" readonly value="${esc(url)}" aria-label="Alerts address">
          <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(url)}">Copy</button></div></details>
    </div>`;
  },
  // OBS's WebSocket: how it is connected, with Try again, and what it's
  // for. Looking here connects it for a minute, like picking a scene.
  obsHtml(){
    const users = S.data.integrations.filter(i => i.enabled && usesObs(i)).map(i => i.name);
    return `<div class="sys-section ig-form" data-obs-live>${obsStatusHtml()}
      <div class="ig-insp-p">Scene triggers and OBS steps talk to OBS through its WebSocket server, built into OBS 28 and later. Switch it on once in OBS: <b>Tools › WebSocket Server Settings › Enable WebSocket server</b>, then OK. InstantClone reads the port and password from OBS itself.</div>
      <div class="muted">${users.length ? `Kept connected for ${esc(users.slice(0, 3).join(', '))}${users.length > 3 ? ` and ${users.length - 3} more` : ''}.`
        : 'Nothing uses OBS yet, so InstantClone only stays connected while the dashboard is open.'}</div></div>`;
  },
  discordHtml(){
    const e = this.editing;
    const rows = S.data.connections.discord.map(c => `<div class="ig-acct" data-key="d-${esc(c.id)}"><span class="dcard-icon ig-ic-discord">${svg('bubble')}</span>
      <div class="dcard-id"><div class="dcard-name" translate="no">#${esc(c.name)}</div><div class="dcard-host">${esc(c.hint)}</div></div>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="d-test" data-id="${esc(c.id)}">Test</button>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="d-edit" data-id="${esc(c.id)}">Edit</button>
      <button class="ic-btn-tiny" data-act="d-del" data-id="${esc(c.id)}" aria-label="Remove #${esc(c.name)}">${svg('x')}</button></div>`).join('');
    const form = e ? `<div class="sys-section ig-form" data-key="d-form-${esc(e.id || 'new')}">
        <div class="ic-label">${e.id ? 'Edit channel' : 'New channel'}</div>
        <div class="dfg"><div class="dff"><label>Name</label><input class="ic-input" data-bind="d-name" value="${esc(e.name)}" placeholder="Mods" autofocus></div>
        ${secretField('d-url', `<input class="ic-input mono ig-secret" data-bind="d-url" value="" placeholder="${e.id ? 'leave blank to keep it' : 'https://discord.com/api/webhooks/…'}" spellcheck="false" autocomplete="off">`, 'Webhook link')}</div>
        <div class="muted">In Discord: Server settings › Integrations › Webhooks › New webhook › Copy webhook URL.</div>
        <div class="ig-form-actions"><button class="ic-btn ic-btn-ghost" data-act="d-cancel">Cancel</button><button class="ic-btn ic-btn-primary" data-act="d-save">Save channel</button></div>
      </div>` : `<button class="dest-add ig-add-row" data-act="d-new" data-key="d-add"><span class="dest-add-icon">${svg('plus')}</span>Add a Discord channel</button>`;
    return `<div class="ig-accts" data-flip>${rows || '<div class="muted ig-none" data-key="d-none">No Discord channel yet.</div>'}${form}</div>`;
  },
  twitchHtml(){
    const t = S.data.twitch;
    if (!t.available){
      return warnHtml('This copy of InstantClone has no Twitch app, so it can\'t log in to Twitch yet. '
        + 'You can add your own in a couple of minutes: <a href="#" data-act="twitch-app">System › Twitch login app</a>.');
    }
    const other = S.data.twitch_other_channel && t.main.login
      ? warnHtml(`You're logged in as <b>${esc(t.main.login)}</b>, but your Twitch destination streams to another channel. `
        + `VOD markers and clips go to ${esc(t.main.login)}'s channel, and fail while it's offline. `
        + 'To use them on the channel you stream to, disconnect and log in with that account.')
      : '';
    const flow = t.login, logging = flow && flow.which === 'main';
    const missing = twitchMissing() ? `<div class="lan-warn ig-notice" data-key="t-missing"><span class="lw-ic">⚠</span>
        <span class="ig-notice-text">Your login is from before InstantClone asked for <b>${esc(twitchMissing())}</b>: until you log in again, those fail. Same account; everything else keeps working.</span>
        ${logging ? '' : '<button class="ic-btn ic-btn-primary ig-small" data-act="t-login" data-which="main">Log in again</button>'}</div>` : '';
    return (t.notice ? warnHtml(esc(t.notice)) : '') + other + missing
      + this.accountHtml('main', t.main, 'Your Twitch account', 'Log in once: InstantClone keeps it fresh.')
      + this.twitchEventsHtml(t)
      + this.accountHtml('bot', t.bot, 'Bot account (optional)', 'A second account that posts in chat instead of you.')
      + `<label class="crash-check"><input type="checkbox" data-act="t-shared" ${S.data.twitch_shared_chat ? 'checked' : ''}>
        <span><span class="crash-check-title">Shared Chat: listen to partner channels too</span>
        <span class="muted">When you share chat with other streamers, their viewers can use your commands and count toward "chat gets busy". Off: only your own chat does.</span></span></label>`
      + '<div class="muted">No password ever goes through InstantClone: you approve it on twitch.tv, and can revoke it there any time.'
      + (t.app === 'custom' ? ' Logging in through your own Twitch app (<a href="#" data-act="twitch-app">change</a>).' : '') + '</div>';
  },
  // What Twitch events integrations hear, and an Allow when the login
  // predates them. Logging in again asks Twitch for the new permissions.
  twitchEventsHtml(t){
    const ev = t.events || {}, subs = ev.subs || {};
    if (!t.main.login || !Object.keys(subs).length) return '';
    const hearing = twitchEventsIn('listening'), consent = twitchEventsIn('needs_consent');
    const flow = S.data.twitch.login, logging = flow && flow.which === 'main';
    return `<div class="sys-section ig-form" data-key="t-events">
      <div class="ic-label">Twitch events</div>
      ${hearing.length ? `<div class="ig-meter"><span class="ig-meter-dot ok"></span><span>Listening for ${esc(twitchNouns(hearing))}</span></div>` : ''}
      ${ev.state === 'error' ? `<div class="ig-meter"><span class="ig-meter-dot off"></span><span>${esc(ev.problem)}</span></div>` : ''}
      ${consent.length && !twitchMissing() ? `<div class="lan-warn ig-notice"><span class="lw-ic">⚠</span>
        <span class="ig-notice-text">Twitch needs your OK for <b>${esc(twitchNouns(consent))}</b>. Your login is from before these events: Allow logs you in again with the same account.</span>
        ${logging ? '' : '<button class="ic-btn ic-btn-primary ig-small" data-act="t-login" data-which="main">Allow on Twitch</button>'}</div>` : ''}
    </div>`;
  },
  accountHtml(which, a, title, blurb){
    const flow = S.data.twitch.login;
    const active = flow && flow.which === which;
    let right;
    if (active){
      right = flow.error
        ? `<button class="ic-btn" data-act="t-login" data-which="${which}">Try again</button>`
        : '<span class="ig-running"><span class="ig-spin"></span>Waiting for you on Twitch…</span>';
    } else if (a.login){
      right = `<span class="dcard-status ${a.chat === 'connected' ? 's-live' : 'ig-flag warn'}"><span class="d-dot"></span><span class="dcard-status-l">chat ${esc(a.chat)}</span></span>
        <button class="ic-btn ic-btn-ghost ig-small" data-act="t-logout" data-which="${which}">Disconnect</button>`;
    } else {
      right = `<button class="ic-btn ${which === 'main' ? 'ic-btn-primary' : ''}" data-act="t-login" data-which="${which}">Connect</button>`;
    }
    let code = '';
    if (active && flow.error) code = warnHtml(esc(flow.error));
    else if (active && flow.user_code){
      const uri = /^https:\/\//.test(flow.uri || '') ? flow.uri : 'https://www.twitch.tv/activate';
      code = `<div class="ig-device">
        <div class="ig-code">${esc(flow.user_code)}</div>
        <div class="ig-device-actions">
          <a class="ic-btn ic-btn-primary" href="${esc(uri)}" target="_blank" rel="noopener">Open twitch.tv/activate</a>
          <button class="ic-btn ic-btn-ghost" data-act="copy" data-text="${esc(flow.user_code)}">Copy code</button>
          <button class="ic-btn ic-btn-ghost" data-act="t-cancel">Cancel</button></div>
        <div class="muted">Enter the code there and approve. This page updates on its own. The code works for ${plural(Math.max(1, Math.round(flow.expires_in_s / 60)), 'more minute')}.</div></div>`;
    } else if (active) code = '<div class="ig-running"><span class="ig-spin"></span>Asking Twitch for a code…</div>';
    return `<div class="sys-section ig-account">
      <div class="ig-acct bare"><span class="dcard-icon ig-ic-twitch">${svg('bubble')}</span>
        <div class="dcard-id"><div class="dcard-name">${a.login ? esc(a.login) : title}</div><div class="dcard-host ig-blurb">${blurb}</div></div>${right}</div>${code}</div>`;
  },
  phoneHtml(){
    const p = S.data.connections.phone;
    return `<div class="sys-section ig-form">
      <div class="ig-insp-p">Phone pushes use <b>ntfy</b>, a free app for Android and iPhone. Install it, subscribe to a topic, and put the same topic here. Anyone who knows the topic can read it, so make it hard to guess.</div>
      <div class="dfg">${secretField('p-topic', `<div class="ig-copy-row"><input class="ic-input mono ig-secret" data-bind="p-topic" value="${esc(p.topic)}" placeholder="instantclone-…" spellcheck="false" autocomplete="off">
        <button class="ic-btn ic-btn-ghost ig-small" data-act="p-random" title="Make a hard-to-guess topic">Random</button></div>`, 'Topic')}
        <div class="dff"><label>Server <span class="muted">(optional)</span></label><input class="ic-input mono" data-bind="p-server" value="${esc(p.server)}" placeholder="https://ntfy.sh" spellcheck="false"></div></div>
      <div class="ig-form-actions"><button class="ic-btn ic-btn-ghost" data-act="p-test" ${p.topic ? '' : 'disabled'}>Send a test push</button><button class="ic-btn ic-btn-primary" data-act="p-save">Save</button></div>
    </div>`;
  },
  value(bind){
    const el = q(`[data-bind="${bind}"]`);
    return el ? el.value : '';
  },
  async act(a, el){
    switch (a){
      case 'conn-tab':
        this.tab = this.tab === el.dataset.tab ? null : el.dataset.tab;
        this.editing = null;
        this.render();
        if (this.tab) revealOpened(q(`[data-key="cx-${this.tab}"]`));
        return true;
      case 'd-new': this.editing = {id:'', name:''}; this.render(); focusField('d-name'); return true;
      case 'd-edit': {
        const c = S.data.connections.discord.find(x => x.id === el.dataset.id);
        if (c){ this.editing = {id:c.id, name:c.name}; this.render(); focusField('d-name'); }
        return true;
      }
      case 'd-cancel': this.editing = null; this.render(); return true;
      case 'd-save':
        await busy(el, async () => {
          const r = await api('/connections/discord', {id:this.editing.id, name:this.value('d-name'), url:this.value('d-url')});
          if (!r.ok){ toast(r.error, 'err', 6000); return; }
          this.editing = null;
          await refreshData();
          this.render();
          toast('Channel saved', 'ok');
        });
        return true;
      case 'd-del':
        if (!armConfirm(el, 'Remove?')) return true;
        await busy(el, async () => {
          const r = await api('/connections/discord/delete', {id:el.dataset.id});
          if (!r.ok){ toast(r.error || 'Could not remove it', 'err', 6000); return; }
          await refreshData();
          this.render();
          toast('Channel removed. Integrations posting there are off until you pick another.', 'info', 5000);
        });
        return true;
      case 'd-test':
        await busy(el, async () => {
          const r = await api('/connections/discord/test', {id:el.dataset.id});
          toast(r.ok ? 'Sent a test message, check Discord' : r.error, r.ok ? 'ok' : 'err', 5000);
        });
        return true;
      case 't-login':
        await busy(el, async () => { await api('/twitch/login', {which:el.dataset.which}); await refreshData(); schedulePoll(); this.render(); });
        return true;
      case 't-cancel':
        await busy(el, async () => { await api('/twitch/cancel', {}); await refreshData(); this.render(); });
        return true;
      case 't-shared': {
        const r = await api('/twitch/shared-chat', {on:el.checked});
        if (!r.ok){ el.checked = !el.checked; toast(r.error || 'Could not save it', 'err'); return true; }
        await refreshData();
        toast(el.checked ? 'Partner channels\' viewers can use your commands' : 'Only your own chat counts', 'ok');
        return true;
      }
      case 't-logout':
        if (!armConfirm(el, 'Chat stops. Sure?')) return true;
        await busy(el, async () => {
          await api('/twitch/logout', {which:el.dataset.which});
          await new Promise(r => setTimeout(r, 400));
          await refreshData();
          this.render();
        });
        return true;
      case 'p-random': {
        const f = q('[data-bind="p-topic"]');
        if (f){
          // A new topic has to be typed into the phone app, so show it.
          S.revealed.add('p-topic');
          this.render();
          const fresh = q('[data-bind="p-topic"]');
          fresh.value = 'instantclone-' + randomToken().slice(0, 12);
          decodeField(fresh, true);
        }
        return true;
      }
      case 'p-save':
        await busy(el, async () => {
          const r = await api('/connections/phone', {topic:this.value('p-topic'), server:this.value('p-server')});
          if (!r.ok){ toast(r.error, 'err', 6000); return; }
          await refreshData();
          this.render();
          toast('Phone saved', 'ok');
        });
        return true;
      case 'p-test':
        await busy(el, async () => {
          const r = await api('/connections/phone/test', {});
          toast(r.ok ? 'Sent, check your phone' : r.error, r.ok ? 'ok' : 'err', 5000);
        });
        return true;
    }
    return false;
  },
};

