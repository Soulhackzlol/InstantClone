// Back from a language change (reloadForLanguage): before the first paint, say
// so, and how tall the wizard was, so the page comes back as it left. Two
// seconds is the backstop should the page never say it's ready.
try {
  const back = JSON.parse(sessionStorage.getItem('instantclone_lang_return'));
  sessionStorage.removeItem('instantclone_lang_return');
  if (back){
    const root = document.documentElement;
    root.dataset.langReturn = back.view;
    root.style.setProperty('--lang-return-h', back.height + 'px');
    window.IC_LANG_BACK = back;
    setTimeout(() => { if (root.dataset.langReturn) root.classList.add('lang-ready'); }, 2000);
  }
} catch (_){}
