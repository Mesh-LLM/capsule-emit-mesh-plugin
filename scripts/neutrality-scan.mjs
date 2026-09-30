#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Neutrality check: fails if reserved vocabulary appears in any tracked text
// file.
//
// The reserved list is NOT in this repository (a public check that listed the
// terms would publish them). It comes from the NEUTRALITY_TERMS secret, as
// JSON: {"substring": [...], "word": [...], "allow_phrases": [...]}
//   substring      matched case-insensitively anywhere
//   word           matched case-insensitively at word boundaries
//   allow_phrases  already-public sentences that may contain a term; a hit is
//                  exempt only when it lies inside the span of such a phrase
//                  on the same line (so a second occurrence outside it still
//                  counts)
//
// Fails closed: with the secret missing, empty or unusable the check exits 2,
// never 0.
//
// Output is redacted by default. Unless NEUTRALITY_REVEAL=true (set only for
// trusted runs), a failing run prints one constant verdict: no term, no path,
// no line, no count. On a fork pull request the scanned files are the
// submitter's own, so a file:line list would let a stranger submit one
// candidate word per line and read the reserved list back from the line
// numbers in the public log. A constant verdict leaves one bit per run.
//
// Only regular files inside the scanned tree are read: a path whose own name
// or any parent directory is a symlink, or that resolves outside the tree, is
// skipped and never opened, and the open itself refuses a symlink swapped in
// after the checks.
//
// Usage: node scripts/neutrality-scan.mjs [repo-dir]
//        node scripts/neutrality-scan.mjs --self-test
// Exit: 0 clean, 1 reserved vocabulary found, 2 misconfigured.

import { execFileSync } from 'node:child_process'
import { closeSync, constants, fstatSync, lstatSync, openSync, readFileSync, realpathSync } from 'node:fs'
import { isAbsolute, join, relative, sep } from 'node:path'
import { pathToFileURL } from 'node:url'

function escape(term) {
  return term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/**
 * The configuration in NEUTRALITY_TERMS. Some ways of setting a secret store
 * the JSON double-encoded (a JSON string whose value is the object); one extra
 * decode recovers it. Anything that is still not an object is an error.
 */
export function parseTerms(raw) {
  let config = JSON.parse(raw)
  if (typeof config === 'string') config = JSON.parse(config)
  if (config === null || typeof config !== 'object' || Array.isArray(config)) {
    throw new Error('NEUTRALITY_TERMS must be a JSON object')
  }
  return config
}

/** Non-empty strings only: an empty term would match everywhere, and an empty
 *  allow phrase would never advance its search. */
function nonEmpty(list) {
  return (list ?? []).filter((t) => typeof t === 'string' && t.length > 0)
}

export function compile(config) {
  const substring = nonEmpty(config.substring)
  const word = nonEmpty(config.word)
  if (substring.length === 0 && word.length === 0) {
    throw new Error('NEUTRALITY_TERMS carries no terms')
  }
  const parts = [
    ...substring.map(escape),
    ...word.map((t) => `\\b${escape(t)}\\b`),
  ]
  return {
    pattern: new RegExp(parts.join('|'), 'gi'),
    allow: nonEmpty(config.allow_phrases).map((p) => p.toLowerCase()),
  }
}

export function lineHits(line, { pattern, allow }) {
  const lower = line.toLowerCase()
  const spans = []
  for (const phrase of allow) {
    if (phrase.length === 0) continue
    let from = 0
    for (;;) {
      const at = lower.indexOf(phrase, from)
      if (at < 0) break
      spans.push([at, at + phrase.length])
      from = at + 1
    }
  }
  const hits = []
  pattern.lastIndex = 0
  for (const m of line.matchAll(pattern)) {
    const start = m.index
    const end = start + m[0].length
    if (spans.some(([s, e]) => s <= start && end <= e)) continue
    hits.push(m[0])
  }
  return hits
}

/**
 * Whether `file` (a path relative to `root`, as git lists it) is a regular
 * file reached without following a symlink: neither it nor any directory
 * on the way is a symlink, and its real path lies inside the real root.
 * Under pull_request_target the checked-out tree is a stranger's; a symlink
 * to a file outside it (a credentials file, /proc/self/environ) must never
 * be opened. A symlink is skipped, not scanned.
 */
export function isPlainFileInside(root, file) {
  if (isAbsolute(file) || file.split(/[\\/]/).includes('..')) return false
  const parts = file.split('/')
  try {
    let at = root
    for (const part of parts) {
      at = join(at, part)
      if (lstatSync(at).isSymbolicLink()) return false
    }
    if (!lstatSync(at).isFile()) return false
    const rel = relative(realpathSync(root), realpathSync(at))
    return rel !== '' && !rel.startsWith('..' + sep) && rel !== '..' && !isAbsolute(rel)
  } catch {
    return false
  }
}

/**
 * The bytes of `file`, opened without following a symlink at its last
 * component and checked to be a regular file after opening, so a link swapped
 * in after isPlainFileInside() is not followed either. Null when unreadable.
 */
export function readPlainFile(root, file) {
  if (!isPlainFileInside(root, file)) return null
  let fd
  try {
    fd = openSync(join(root, file), constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0))
  } catch {
    return null
  }
  try {
    if (!fstatSync(fd).isFile()) return null
    return readFileSync(fd)
  } catch {
    return null
  } finally {
    closeSync(fd)
  }
}

export function scan(root, compiled) {
  const files = execFileSync('git', ['ls-files', '-z'], { cwd: root, stdio: ['ignore', 'pipe', 'ignore'] })
    .toString('utf8')
    .split('\0')
    .filter(Boolean)
  const offenders = []
  for (const file of files) {
    const buffer = readPlainFile(root, file)
    // Every file is scanned as text, NUL bytes included: skipping files that
    // look binary would let a pull request hide a term behind one NUL byte.
    if (buffer === null) continue
    buffer
      .toString('utf8')
      .split(/\r?\n/)
      .forEach((line, i) => {
        const hits = lineHits(line, compiled)
        if (hits.length > 0) offenders.push({ where: `${file}:${i + 1}`, hits })
      })
  }
  return offenders
}

function selfTest() {
  const compiled = compile({ substring: ['reserved'], allow_phrases: ['use of reserved keyword'] })
  const cases = [
    ['reserved word found here', 1],
    ['see use of reserved keyword in docs', 0],
    ['reserved is bad but use of reserved keyword is documented', 1],
    ['RESERVED in capitals', 1],
  ]
  const wordOnly = compile({ word: ['ab'] })
  cases.push(['a lab report', 0, wordOnly], ['ab alone', 1, wordOnly])
  const failures = cases
    .filter(([line, want, c]) => lineHits(line, c ?? compiled).length !== want)
    .map(([line, want]) => `${JSON.stringify(line)} expected ${want} hit(s)`)
  // Redaction is the default: only the exact value 'true' reveals.
  for (const [value, want] of [[undefined, false], ['', false], ['false', false], ['1', false], ['true', true]]) {
    if (revealRequested(value) !== want) failures.push(`NEUTRALITY_REVEAL=${JSON.stringify(value)} should reveal: ${want}`)
  }
  // The untrusted verdict is constant: it names nothing from the input.
  const a = report([{ where: 'a.md:1', hits: ['x'] }], false)
  const b = report([{ where: 'other/b.md:7', hits: ['y', 'z'] }, { where: 'c.md:2', hits: ['x'] }], false)
  if (a.join('\n') !== b.join('\n') || /a\.md|\bx\b|1/.test(a.join('\n'))) {
    failures.push(`untrusted output varies with the input or names it: ${JSON.stringify(a)} vs ${JSON.stringify(b)}`)
  }
  try {
    const twice = parseTerms(JSON.stringify(JSON.stringify({ substring: ['t'] })))
    if (twice.substring?.[0] !== 't') failures.push('a double-encoded NEUTRALITY_TERMS was not decoded')
  } catch (error) {
    failures.push(`a double-encoded NEUTRALITY_TERMS was refused: ${error.message}`)
  }
  for (const bad of ['[]', '1', 'null', '"x"']) {
    let refused = false
    try {
      parseTerms(bad)
    } catch {
      refused = true
    }
    if (!refused) failures.push(`NEUTRALITY_TERMS=${bad} should be refused`)
  }
  if (failures.length > 0) {
    for (const failure of failures) console.error(`self-test failed: ${failure}`)
    process.exit(1)
  }
  console.log('neutrality self-test: OK (span-based allow phrases; constant verdict unless revealed)')
}

/** Only the exact value 'true', which the workflow sets for trusted runs. */
export function revealRequested(value) {
  return value === 'true'
}

/**
 * The lines to print for a failing scan. Unrevealed, it is one constant line
 * whatever was found: no term, path, line or count (see the header).
 */
export function report(offenders, reveal) {
  if (!reveal) {
    return [
      'neutrality: content check failed. Details are withheld on untrusted runs; ' +
        'a maintainer can re-run this check on a trusted event (a push to main or a manual run) to see them.',
    ]
  }
  return [
    `neutrality: reserved vocabulary found on ${offenders.length} line(s):`,
    ...offenders.map(({ where, hits }) => `  ${where}: ${hits.map((h) => JSON.stringify(h)).join(', ')}`),
  ]
}

function main(argv) {
  if (argv[0] === '--self-test') return selfTest()
  const raw = (process.env.NEUTRALITY_TERMS ?? '').trim()
  if (!raw) {
    console.error('error: NEUTRALITY_TERMS is empty or unset; this check fails closed.')
    process.exit(2)
  }
  let compiled
  try {
    compiled = compile(parseTerms(raw))
  } catch (error) {
    // JSON.parse's message can quote the input; never echo it.
    console.error(`error: NEUTRALITY_TERMS is not usable (${error instanceof SyntaxError ? 'not valid JSON' : error.message}).`)
    process.exit(2)
  }
  let offenders
  try {
    offenders = scan(argv[0] ?? '.', compiled)
  } catch {
    console.error('error: could not list the tracked files (is the path a git checkout?); this check fails closed.')
    process.exit(2)
  }
  if (offenders.length === 0) {
    console.log('neutrality: clean')
    return
  }
  for (const line of report(offenders, revealRequested(process.env.NEUTRALITY_REVEAL))) console.error(line)
  process.exit(1)
}

// Run only when invoked as a script, so tests can import the functions.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2))
}
