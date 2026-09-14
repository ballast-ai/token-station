import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { desktopBuildInfo } from '../scripts/desktop-build-info.mjs';

function repository(t) {
  const root = mkdtempSync(path.join(tmpdir(), 'ts-build-info-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  git('init');
  git('config', 'user.name', 'Build test');
  git('config', 'user.email', 'build@example.invalid');
  writeFileSync(path.join(root, 'source'), 'original');
  git('add', 'source');
  git('commit', '-m', 'Initial source');
  return { root, git };
}

test('uses checked out source ahead of stale CI metadata', (t) => {
  const { root, git } = repository(t);
  const info = desktopBuildInfo(root, { env: { GITHUB_SHA: 'abcdef123456', TOKEN_STATION_COMMIT_HASH: '1234567' } });
  assert.equal(info.commit, git('rev-parse', '--short=7', 'HEAD'));
  assert.equal(info.sourceState, 'clean');
  assert.equal(info.channel, 'local');
  assert.match(info.builtAt, /^\d{4}-\d{2}-\d{2}T.*Z$/);
});

test('marks unstaged, staged, and untracked source as modified without leaking content', (t) => {
  const { root, git } = repository(t);
  writeFileSync(path.join(root, 'source'), 'private-test-value');
  assert.equal(desktopBuildInfo(root).sourceState, 'modified');
  git('add', 'source');
  assert.equal(desktopBuildInfo(root).sourceState, 'modified');
  git('commit', '-m', 'Changed source');
  writeFileSync(path.join(root, 'private-filename'), 'private-test-value');
  const info = desktopBuildInfo(root);
  assert.equal(info.sourceState, 'modified');
  assert.doesNotMatch(JSON.stringify(info), /private-|ts-build-info-/);
});

test('reports missing Git state honestly and only accepts a bounded hexadecimal fallback', () => {
  const root = path.join(tmpdir(), 'ts-missing-build-root');
  assert.deepEqual(desktopBuildInfo(root, { env: { GITHUB_SHA: '<script>' } }).sourceState, 'unknown');
  assert.equal(desktopBuildInfo(root, { env: { GITHUB_SHA: '<script>' } }).commit, 'unknown');
  assert.equal(desktopBuildInfo(root, { env: { GITHUB_SHA: 'abcdef123456' } }).commit, 'abcdef1');
});

test('separates local, development, preview, and production build channels', (t) => {
  const { root } = repository(t);
  for (const channel of ['local', 'preview', 'production']) {
    assert.equal(desktopBuildInfo(root, { env: { TOKEN_STATION_BUILD_CHANNEL: channel } }).channel, channel);
  }
  assert.equal(desktopBuildInfo(root, { env: {}, command: 'serve' }).channel, 'development');
  assert.equal(desktopBuildInfo(root, { env: { TOKEN_STATION_BUILD_CHANNEL: 'secret-value' } }).channel, 'unknown');
});


test('does not mistake an enclosing repository for the source archive identity', (t) => {
  const { root, git } = repository(t);
  writeFileSync(path.join(root, '.gitignore'), 'archive/\n');
  git('add', '.gitignore');
  git('commit', '-m', 'Ignore the source archive');
  const archive = path.join(root, 'archive');
  mkdirSync(archive);
  const info = desktopBuildInfo(archive, { env: { TOKEN_STATION_COMMIT_HASH: 'abcdef123456' } });
  assert.equal(info.commit, 'abcdef1');
  assert.equal(info.sourceState, 'unknown');
});
