// Reads a page file the way the build serves it. The dashboard (index.html)
// and the Integrations script (integrations.js) are kept as pieces: build.rs
// builds each from its skeleton, replacing every include line with the piece
// it names. This does the same for the tests, by the same rules as
// src/pieced.rs (which has the full story): a change there belongs here too.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const WEB = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

/** Files served whole but kept as pieces: served name -> skeleton under web/ (build.rs `PIECED`). */
const PIECED = { 'index.html': 'dashboard/index.html', 'integrations.js': 'integrations/integrations.js' };

const PIECE_EXTENSIONS = ['.html', '.css', '.js'];
const INCLUDE_OPEN = '<!-- include ';
const INCLUDE_CLOSE = ' -->';

/** Whether a file in a skeleton's folder is a piece (pieced.rs `is_piece_name`). */
export function isPieceName(file) {
  const name = file.split('/').pop();
  return !name.startsWith('.') && PIECE_EXTENSIONS.some(ext => name.endsWith(ext));
}

/** A path an include line can name (pieced.rs `is_plain_piece_path`). */
function isPlainPiecePath(file) {
  return file.split('/').every(step => step !== '.' && step !== '..' && /^[A-Za-z0-9._-]+$/.test(step))
    && isPieceName(file);
}

/** The piece an include line names, null for any other line; throws for a near miss (pieced.rs `include_target`). */
export function includeTarget(line) {
  const trimmed = line.trim();
  if (!trimmed.startsWith('<!--')) return null;
  if (!/^include(\s|$)/.test(trimmed.slice(4).trimStart())) return null;
  const file = trimmed.startsWith(INCLUDE_OPEN) && trimmed.endsWith(INCLUDE_CLOSE)
    ? trimmed.slice(INCLUDE_OPEN.length, trimmed.length - INCLUDE_CLOSE.length) : '';
  if (!isPlainPiecePath(file)) {
    throw new Error(`"${trimmed}" is not an include line: write ${INCLUDE_OPEN}folder/name.css${INCLUDE_CLOSE} `
      + 'with single spaces, a path inside the skeleton\'s folder (letters, digits, . _ -, / between folders) '
      + 'and a .html, .css or .js name');
  }
  return file;
}

/** The pieces in a skeleton's folder, sorted (pieced.rs `pieces_in`). */
export function piecesIn(dir, skeletonName) {
  const pieces = fs.readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter(entry => entry.isFile())
    .map(entry => path.relative(dir, path.join(entry.parentPath ?? entry.path, entry.name)).split(path.sep).join('/'))
    .filter(file => file !== skeletonName && isPieceName(file));
  const odd = pieces.find(file => !isPlainPiecePath(file));
  if (odd) throw new Error(`${odd} (next to ${skeletonName}): rename it to letters, digits and . _ - only, so an include line can name it`);
  return pieces.sort();
}

const linesOf = text => text.match(/[^\n]*\n|[^\n]+$/g) ?? [];

/** A piece as it goes into the served file (pieced.rs `prepare_piece`). */
function preparePiece(name, text) {
  if (text.startsWith('﻿')) text = text.slice(1);
  text.split('\n').forEach((line, index) => {
    let target = null;
    try { target = includeTarget(line); } catch { /* a near miss inside a piece is just text */ }
    if (target) throw new Error(`${name}:${index + 1}: an include line only works in the skeleton`);
  });
  return text && !text.endsWith('\n') ? `${text}\n` : text;
}

/**
 * A skeleton built into its served file (pieced.rs `join`). Returns the
 * text and `origins`: for each line of it, where that line comes from, as
 * `web/<folder>/<piece>:<line>` (or the skeleton's own line).
 */
export function joinSkeleton(skeletonPath) {
  const dir = path.dirname(skeletonPath);
  const label = path.relative(path.dirname(WEB), skeletonPath).split(path.sep).join('/');
  const pieces = piecesIn(dir, path.basename(skeletonPath));
  const folderLabel = path.dirname(label);
  const included = new Set();
  let text = '';
  const origins = [];
  linesOf(fs.readFileSync(skeletonPath, 'utf8').replace(/^﻿/, '')).forEach((line, index) => {
    const at = `${label}:${index + 1}`;
    let piece;
    try { piece = includeTarget(line); } catch (error) { throw new Error(`${at}: ${error.message}`); }
    if (!piece) { text += line; origins.push(at); return; }
    if (!pieces.includes(piece)) throw new Error(`${at}: includes ${piece}, but there is no such piece (names are case-sensitive)`);
    if (included.has(piece)) throw new Error(`${at}: includes ${piece} a second time`);
    const body = preparePiece(piece, fs.readFileSync(path.join(dir, piece), 'utf8'));
    linesOf(body).forEach((_, n) => origins.push(`${folderLabel}/${piece}:${n + 1}`));
    text += body;
    included.add(piece);
  });
  const forgotten = pieces.filter(piece => !included.has(piece));
  if (forgotten.length) throw new Error(`${label} has no include line for ${forgotten.join(', ')} (every piece in its folder must be included)`);
  return { text, origins };
}

/** Built pieced files, once per test run: the sources don't change while tests read them. */
const built = new Map();
function builtFile(name) {
  if (!built.has(name)) built.set(name, joinSkeleton(path.join(WEB, PIECED[name])));
  return built.get(name);
}

/** A file under web/ as served: a pieced one comes back built from its skeleton. */
export function readWebFile(name) {
  return PIECED[name] ? builtFile(name).text : fs.readFileSync(path.join(WEB, name), 'utf8');
}

/**
 * Where line `line` (1-based) of served file `name` lives in the source:
 * the piece and its line for a pieced file, `name:line` otherwise.
 */
export function sourceOf(name, line) {
  return (PIECED[name] && builtFile(name).origins[line - 1]) || `${name}:${line}`;
}
