// SPDX-License-Identifier: Apache-2.0
// Tests for the neutrality scanner: it scans tracked regular files, never
// follows a symlink out of the tree, fails closed, and prints nothing that
// varies with the input unless the run is trusted.
// Run: node --test scripts/*.test.mjs

import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { compile, isPlainFileInside, lineHits, readPlainFile, scan } from './neutrality-scan.mjs'

const TERM = 'reservedexample'

/** A git repo with one tracked file containing TERM, a tracked symlink to a
 *  file outside the repo that also contains TERM, and a symlinked directory
 *  pointing outside. */
function fixture() {
  const base = mkdtempSync(join(tmpdir(), 'neutrality-scan-'))
  const outside = join(base, 'outside')
  const repo = join(base, 'repo')
  mkdirSync(outside)
  mkdirSync(repo)
  writeFileSync(join(outside, 'secret.txt'), `${TERM}\n`)
  writeFileSync(join(repo, 'inside.txt'), `a line with ${TERM}\n`)
  writeFileSync(join(repo, 'clean.txt'), 'nothing here\n')
  symlinkSync(join(outside, 'secret.txt'), join(repo, 'link.txt'))
  symlinkSync(outside, join(repo, 'linkdir'))
  const git = (...args) => execFileSync('git', args, { cwd: repo, stdio: 'ignore' })
  git('init', '-q')
  git('add', 'inside.txt', 'clean.txt', 'link.txt', 'linkdir')
  return { repo }
}

test('scans tracked regular files', () => {
  const { repo } = fixture()
  const found = scan(repo, compile({ substring: [TERM] })).map((o) => o.where)
  assert.deepEqual(found, ['inside.txt:1'])
})

test('scans files whose names git would quote: non-ASCII, newline, quote', () => {
  const repo = mkdtempSync(join(tmpdir(), 'neutrality-scan-names-'))
  const names = ['naïve.txt', 'two\nlines.txt', 'say "hi".txt']
  for (const name of names) writeFileSync(join(repo, name), `${TERM}\n`)
  const git = (...args) => execFileSync('git', args, { cwd: repo, stdio: 'ignore' })
  git('init', '-q')
  git('add', '--', ...names)
  const found = scan(repo, compile({ substring: [TERM] })).map((o) => o.where).sort()
  assert.deepEqual(found, names.map((n) => `${n}:1`).sort())
})

test('never reads through a symlink, to a file or through a directory', () => {
  const { repo } = fixture()
  assert.equal(isPlainFileInside(repo, 'inside.txt'), true)
  assert.equal(isPlainFileInside(repo, 'link.txt'), false)
  assert.equal(isPlainFileInside(repo, 'linkdir'), false)
  assert.equal(isPlainFileInside(repo, 'linkdir/secret.txt'), false)
  assert.equal(isPlainFileInside(repo, '../outside/secret.txt'), false)
  assert.equal(isPlainFileInside(repo, '/etc/hosts'), false)
  assert.equal(isPlainFileInside(repo, 'missing.txt'), false)
})

/** Runs the CLI on `root`; returns { status, stdout, stderr }. */
function cli(root, env) {
  try {
    const stdout = execFileSync('node', [join(import.meta.dirname, 'neutrality-scan.mjs'), root], {
      env: { ...process.env, NEUTRALITY_REVEAL: '', ...env },
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    return { status: 0, stdout, stderr: '' }
  } catch (error) {
    return { status: error.status, stdout: error.stdout, stderr: error.stderr }
  }
}

const TERMS = JSON.stringify({ substring: [TERM] })

test('an untrusted run prints one constant verdict: no term, path, line or count', () => {
  const { repo } = fixture()
  const result = cli(repo, { NEUTRALITY_TERMS: TERMS })
  assert.equal(result.status, 1)
  const out = result.stdout + result.stderr
  assert.doesNotMatch(out, new RegExp(TERM, 'i'))
  assert.doesNotMatch(out, /inside|link|\.txt|\d/)

  // A different tree (other name, line, count) gives byte-identical output.
  const other = mkdtempSync(join(tmpdir(), 'neutrality-scan-other-'))
  writeFileSync(join(other, 'wholly-different.md'), `clean\n${TERM} and ${TERM}\n${TERM}\n`)
  execFileSync('git', ['init', '-q'], { cwd: other, stdio: 'ignore' })
  execFileSync('git', ['add', '-A'], { cwd: other, stdio: 'ignore' })
  const second = cli(other, { NEUTRALITY_TERMS: TERMS })
  assert.equal(second.status, 1)
  assert.equal(second.stdout + second.stderr, out)
})

test('a trusted run names the plain file and the term, and never a symlinked one', () => {
  const { repo } = fixture()
  const result = cli(repo, { NEUTRALITY_TERMS: TERMS, NEUTRALITY_REVEAL: 'true' })
  assert.equal(result.status, 1)
  assert.match(result.stderr, new RegExp(`inside\\.txt:1: "${TERM}"`))
  assert.doesNotMatch(result.stderr, /link/)
})

test('readPlainFile refuses a symlink and reads a plain file', () => {
  const { repo } = fixture()
  assert.equal(readPlainFile(repo, 'link.txt'), null)
  assert.equal(readPlainFile(repo, 'linkdir/secret.txt'), null)
  assert.equal(readPlainFile(repo, 'inside.txt').toString('utf8'), `a line with ${TERM}\n`)
})

test('fails closed (exit 2) on a missing, malformed or empty term list, or a non-checkout', () => {
  const { repo } = fixture()
  assert.equal(cli(repo, { NEUTRALITY_TERMS: '' }).status, 2)
  assert.equal(cli(repo, { NEUTRALITY_TERMS: '[]' }).status, 2)
  assert.equal(cli(repo, { NEUTRALITY_TERMS: '{}' }).status, 2)
  const bad = cli(repo, { NEUTRALITY_TERMS: `{"substring": ["${TERM}"` })
  assert.equal(bad.status, 2)
  assert.doesNotMatch(bad.stderr, new RegExp(TERM, 'i'))
  const notGit = mkdtempSync(join(tmpdir(), 'neutrality-scan-nogit-'))
  assert.equal(cli(notGit, { NEUTRALITY_TERMS: TERMS }).status, 2)
})

test('accepts a double-encoded term list', () => {
  const { repo } = fixture()
  assert.equal(cli(repo, { NEUTRALITY_TERMS: JSON.stringify(TERMS) }).status, 1)
})

test('a file with a NUL byte is still scanned', () => {
  const repo = mkdtempSync(join(tmpdir(), 'neutrality-scan-nul-'))
  writeFileSync(join(repo, 'hidden.txt'), Buffer.concat([Buffer.from([0]), Buffer.from(`\n${TERM}\n`)]))
  execFileSync('git', ['init', '-q'], { cwd: repo, stdio: 'ignore' })
  execFileSync('git', ['add', '-A'], { cwd: repo, stdio: 'ignore' })
  const found = scan(repo, compile({ substring: [TERM] })).map((o) => o.where)
  assert.deepEqual(found, ['hidden.txt:2'])
})

test('empty terms and empty allow phrases are ignored, not looped on', () => {
  const compiled = compile({ substring: [TERM, ''], word: [''], allow_phrases: [''] })
  assert.deepEqual(lineHits(`x ${TERM} y`, compiled), [TERM])
  assert.deepEqual(lineHits('nothing here', compiled), [])
  // Called directly with an empty phrase (bypassing compile), it still returns.
  assert.deepEqual(lineHits(`${TERM}`, { pattern: new RegExp(TERM, 'gi'), allow: [''] }), [TERM])
  assert.throws(() => compile({ substring: [''], word: [''] }), /no terms/)
})
