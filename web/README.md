# The web pages

Everything the app shows in a browser lives here. The Rust app embeds these
files at build time (gzipped, see [build.rs](../build.rs)) and serves them from
memory, so a change here needs a `cargo build` before you see it.

There is no framework, bundler or npm install: plain HTML, CSS and JavaScript,
and the page tests run on Node alone.

## What is where

| Served at | Source | What it is |
|---|---|---|
| `/` | [dashboard/](dashboard/) | The dashboard, kept as pieces (below) |
| `/integrations.js` | [integrations/](integrations/) | The Integrations tab's script, kept as pieces |
| `/dock` | [dock.html](dock.html) + [dock.js](dock.js) | The OBS dock |
| `/alerts` | [alerts.html](alerts.html) | The on-stream alerts page |
| `/overlay-runtime.js` | [overlay-runtime.js](overlay-runtime.js) | Overlay presets, widgets and the baker |
| `/studio.js` | [studio.js](studio.js) | The Overlay Studio editor |
| `/i18n.js` | [i18n.js](i18n.js) + [i18n/](i18n/) | The translator and the Spanish strings |
| | [tests/](tests/) | The page tests |

## How the dashboard is built

The dashboard is one page with one `<style>` and one `<script>`, about
870 KB of source. To keep it workable it is split into **pieces**: small
files, one folder per feature.

```
dashboard/
  index.html          the skeleton: the page's own tags + one include line per piece
  page/               what the whole page shares: tokens, atoms, fields, tabs, toasts, start-up
  destinations/       destinations.css  destinations.html  destinations.js
  hero/               the big status card (+ the phone bar)
  system/             the System tab, one piece per sub-tab or feature
  ...                 one folder per tab or feature
```

[dashboard/index.html](dashboard/index.html) is the **skeleton**. Where a piece
goes, it has a line like this:

```html
<!-- include destinations/destinations.css -->
```

At build time each include line is replaced by the piece's text, and the
result is the page that is served. Lines that are not include lines pass
through as they are. The Integrations script works the same way, from
[integrations/integrations.js](integrations/integrations.js).

The build is strict, so mistakes stop it with a file and line to fix:

- a piece with no include line ("no include line for x.js")
- an include line for a file that is not there (case counts)
- a piece included twice
- a line that looks like an include line but is written slightly wrong

## The three lists and what their order means

The skeleton has three lists of include lines, and each list's order means
something different. A comment above each list says so.

1. **Styles: shared foundations first, then features in on-screen order.**
   `page/` pieces come first (tokens, atoms, fields...), then each feature's
   pieces, kept together. CSS order *is* the cascade: when two rules are
   equally specific, the later one wins. So a feature can restyle a shared
   atom, never the other way round.
2. **Markup: on-screen order.** The page reads top to bottom like the skeleton.
3. **Scripts: run order.** All pieces become one script. Functions are
   hoisted, so a function can live in any piece. But code that runs at load
   (a `const` with a value, an `addEventListener`, a call) runs top to bottom,
   so it must come after everything it uses. Two pieces are fixed:
   `page/dom.js` is first (its `'use strict';` must open the script) and
   `page/start.js` is last (it starts the live feeds).

A test ([tests/web-files.test.mjs](tests/web-files.test.mjs)) holds these
rules: `page/` styles first, each feature's styles together, `page/dom.js`
first and `page/start.js` last (and `init.js` last in Integrations).

## Recipes

**Change something.** Find the piece by feature folder, or search `web/` for a
class name or a word on screen. Edit it, `cargo build`, reload the page.

**Add a style piece.** Create `dashboard/<feature>/<name>.css` and add its
include line inside `<style>`, next to its feature's other style pieces. A new
feature goes where it shows on screen. Comments inside a style piece are CSS
comments (`/* */`), since the piece lands inside `<style>`.

**Add markup.** Create `<feature>/<name>.html` and add its include line in the
body list, where it shows on screen.

**Add a script piece.** Create `<feature>/<name>.js` and add its include line
in the script list. If it only defines functions, put it next to its feature.
If it runs code at load, it must come after every piece that code uses; when
in doubt, just above `page/start.js`. Use `//` comments.

**Add a tab.** A button in [page/tabs.html](dashboard/page/tabs.html)
(`data-tab="name"`), a `<section class="tab-pane" data-pane="name" hidden>` in
the feature's markup piece, and its include line between its neighbours'.
`showTab('name')` then switches to it.

**Add a word the user sees.** Write it in English in the code. The
translation test then asks for it in [i18n/es.json](i18n/es.json), or in
[i18n/untranslated.txt](i18n/untranslated.txt) if it stays the same (a name,
a key, code). `node web/tests/strings.mjs missing` lists what is lacking.

**Add a piece to Integrations.** The same rules apply inside
[integrations/integrations.js](integrations/integrations.js): the pieces run
in order inside one function, `init.js` last.

## Conventions

- **Fonts:** `var(--sans)` and `var(--mono)` (Windows' own Segoe UI and
  Cascadia Mono / Consolas, no web fonts). Overlays and alerts keep the fonts
  people pick, since they go on stream.
- **Colours and sizes:** the tokens in
  [page/tokens.css](dashboard/page/tokens.css). Reuse the atoms (`.ic-btn`,
  `.ic-input`, `.dcard`, `.ic-modal` + `.dest-form`) before writing new ones.
- **Class names:** prefixed by feature (`.ig-` Integrations, `.st-` Studio,
  `.dest-` Destinations...). A short generic name in two features means one
  silently restyles the other.
- **Animations that loop** move only `transform`, `opacity` or `filter`, which
  the GPU runs off the main thread. A looping `box-shadow` or `background`
  repaints every frame; the page tests refuse it.
- **User text** goes through `esc()` before it touches the DOM.
- **Secrets** (keys, webhooks, tokens) are masked on screen until "Show".

## Checking your change

```
node scripts/check.mjs                      everything CI runs, in one terminal
node scripts/check.mjs --only "Page tests"  just the page tests
node --test "web/tests/*.test.mjs"          the page tests, plain output
```

The page tests load the real scripts into a stand-in page, so they test what
ships. To find which piece a line of the built page came from (a test failure
or a browser error points at the served file), `sourceOf('index.html', line)`
in [tests/web-files.mjs](tests/web-files.mjs) answers with the piece and line.
