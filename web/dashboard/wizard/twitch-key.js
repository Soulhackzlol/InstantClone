// "Get my key from Twitch" on the wizard's key step. The streamer approves
// once on twitch.tv and InstantClone reads the key and puts it in the Twitch
// destination itself (see src/twitch_key.rs): the key never comes through
// this page, and nobody has to find the right dashboard page, which needs
// the channel name we don't know yet.
const TWITCH_KEY_ASK_EVERY_MS = 2000;
let twitchKeyWaitingNow = false;
let twitchKeyTimer = null;
let twitchKeyAsking = false;

function twitchAppAvailable(){
  return !!(lastConfig && (lastConfig.twitch_app_built_in || lastConfig.twitch_client_id));
}

// The key step's Twitch block: shown for Twitch when this copy has a Twitch
// app, and not when Streamlabs already handed the key over.
function syncWizTwitchKey(){
  const box = $('w-tw-key');
  if (!box) return;
  const twitch = $('w-platform').value === 'twitch';
  box.hidden = !twitch || !twitchAppAvailable() || wizKeyFrom === 'Streamlabs';
  const done = wizKeyFrom === 'Twitch';
  box.querySelector('.w-tw-key-row').hidden = twitchKeyWaitingNow;
  $('w-tw-key-wait').hidden = !twitchKeyWaitingNow;
  $('w-tw-key-btn').hidden = done;
  $('w-tw-or').hidden = done;
  $('w-key-verb').textContent = box.hidden ? 'Paste your stream key from' : 'Get your stream key from';
}

/** `kind` is '' (a hint), 'ok' or 'err'. `text` is one sentence or a list,
 * each in its own span so each translates on its own. */
function twitchKeySay(text, kind){
  const msg = $('w-tw-key-msg');
  msg.replaceChildren(...[].concat(text).map((sentence, n) => {
    const span = document.createElement('span');
    span.textContent = (n ? ' ' : '') + sentence;
    return span;
  }));
  msg.classList.toggle('ok', kind === 'ok');
  msg.classList.toggle('err', kind === 'err');
}

async function wizTwitchKeyStart(){
  // Opened inside the click, so the browser doesn't take it for a popup;
  // it goes to Twitch once InstantClone has the code.
  const tab = window.open('', '_blank');
  if (tab) tab.opener = null;
  const btn = $('w-tw-key-btn');
  btn.disabled = true;
  const r = await postAction('/twitch-key/start');
  btn.disabled = false;
  if (!r.ok){
    if (tab) tab.close();
    twitchKeySay(r.error, 'err');
    return;
  }
  const uri = (r.data.uri || '').startsWith('https://') ? r.data.uri : 'https://www.twitch.tv/activate';
  if (tab) tab.location.href = uri;
  $('w-tw-key-open').href = uri;
  $('w-tw-key-code').textContent = r.data.user_code;
  setTwitchKeyWaiting(true);
}

async function wizTwitchKeyCancel(){
  setTwitchKeyWaiting(false);
  twitchKeySay('Approve once on Twitch and your key fills in by itself. Nothing to copy.');
  await postAction('/twitch-key/cancel');
}

function setTwitchKeyWaiting(on){
  twitchKeyWaitingNow = on;
  clearTimeout(twitchKeyTimer);
  twitchKeyTimer = on ? setTimeout(askTwitchKey, TWITCH_KEY_ASK_EVERY_MS) : null;
  syncWizTwitchKey();
}

async function askTwitchKey(){
  if (!twitchKeyWaitingNow || twitchKeyAsking) return;
  clearTimeout(twitchKeyTimer);
  twitchKeyAsking = true;
  const r = await postAction('/twitch-key/check');
  twitchKeyAsking = false;
  if (!twitchKeyWaitingNow) return;   // cancelled meanwhile
  if (r.ok && r.data.state === 'waiting'){
    twitchKeyTimer = setTimeout(askTwitchKey, TWITCH_KEY_ASK_EVERY_MS);
    return;
  }
  setTwitchKeyWaiting(false);
  if (!r.ok){ twitchKeySay(r.error, 'err'); return; }
  wizKeyImported = true;
  wizKeyFrom = 'Twitch';
  $('w-key').value = '';
  twitchKeySay([
    `Got it. The key for ${r.data.login} went straight to your Twitch destination.`,
    r.data.enabled ? '' : 'It\'s switched off: turn it on in Destinations to stream there.',
    r.data.integrations ? 'Twitch is connected in Integrations too, for alerts, chat commands and clips.' : '',
  ].filter(Boolean), 'ok');
  clearWizMsg();
  syncWizStreamlabsImport();
  syncWizTwitchKey();
}

// Back from the Twitch tab: ask now instead of at the next tick.
window.addEventListener('focus', () => { if (twitchKeyWaitingNow) askTwitchKey(); });
