// - OBS > Chat dock: the address of a platform's chat, for an OBS browser dock --
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
