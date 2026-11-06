// Reads a page file the way the build serves it. The dashboard (index.html)
// and the Integrations script (integrations.js) have no file of their own:
// build.rs joins each from the pieces its parts.txt lists, and this does the
// same join for the tests.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const WEB = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

/** Files served whole but kept as pieces: served name -> folder under web/ (build.rs `PIECED`). */
const PIECED = { 'index.html': 'dashboard', 'integrations.js': 'integrations' };

/** The pieces of a pieced file's folder, in the order they are joined. */
function partsOf(folder) {
  return fs.readFileSync(path.join(WEB, folder, 'parts.txt'), 'utf8')
    .split('\n')
    .map(line => line.trim())
    .filter(line => line && !line.startsWith('#'));
}

/** A file under web/ as served: a pieced one comes back joined. */
export function readWebFile(name) {
  const folder = PIECED[name];
  if (!folder) return fs.readFileSync(path.join(WEB, name), 'utf8');
  return partsOf(folder).map(piece => fs.readFileSync(path.join(WEB, folder, piece), 'utf8')).join('');
}
