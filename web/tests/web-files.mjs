// Reads a page file the way the build serves it. The dashboard (index.html)
// and the Integrations script (integrations.js) are kept as pieces: build.rs
// builds each from its skeleton, replacing every include line with the piece
// it names, and this does the same for the tests.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const WEB = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

/** Files served whole but kept as pieces: served name -> skeleton under web/ (build.rs `PIECED`). */
const PIECED = { 'index.html': 'dashboard/index.html', 'integrations.js': 'integrations/integrations.js' };

/** `<!-- include hero/hero.css -->`, alone on its line (build.rs `include_target`). */
const INCLUDE = /^\s*<!-- include (\S+) -->\s*$/;

/** A file under web/ as served: a pieced one comes back built from its skeleton. */
export function readWebFile(name) {
  const skeleton = PIECED[name];
  if (!skeleton) return fs.readFileSync(path.join(WEB, name), 'utf8');
  const folder = path.dirname(path.join(WEB, skeleton));
  const lines = fs.readFileSync(path.join(WEB, skeleton), 'utf8').match(/[^\n]*\n|[^\n]+$/g) ?? [];
  return lines.map(line => {
    const piece = INCLUDE.exec(line);
    return piece ? fs.readFileSync(path.join(folder, piece[1]), 'utf8') : line;
  }).join('');
}
