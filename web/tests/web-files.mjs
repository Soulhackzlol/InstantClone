// Reads a page file the way the build serves it. The dashboard (index.html)
// has no file of its own: build.rs joins it from the pieces that
// web/dashboard/parts.txt lists, and this does the same join for the tests.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const WEB = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const DASHBOARD = path.join(WEB, 'dashboard');

/** The pieces of the dashboard page, in the order they are joined. */
export function dashboardParts() {
  return fs.readFileSync(path.join(DASHBOARD, 'parts.txt'), 'utf8')
    .split('\n')
    .map(line => line.trim())
    .filter(line => line && !line.startsWith('#'));
}

/** A file under web/ as served: `index.html` is the joined dashboard. */
export function readWebFile(name) {
  if (name === 'index.html') {
    return dashboardParts().map(piece => fs.readFileSync(path.join(DASHBOARD, piece), 'utf8')).join('');
  }
  return fs.readFileSync(path.join(WEB, name), 'utf8');
}
