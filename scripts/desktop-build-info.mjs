import { execFileSync } from 'node:child_process';
import { realpathSync } from 'node:fs';

/** Report source identity without exposing repository paths or file contents. */
export function desktopBuildInfo(root, { env = process.env, command = 'build' } = {}) {
  const git = (...args) => execFileSync('git', args, {
    cwd: root,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'ignore'],
  }).trim();
  let commit = 'unknown';
  let sourceState = 'unknown';
  try {
    if (realpathSync(git('rev-parse', '--show-toplevel')) !== realpathSync(root)) {
      throw new Error('Source root does not match the Git working tree');
    }
    const actual = git('rev-parse', '--short=7', 'HEAD');
    if (/^[0-9a-f]{7,40}$/i.test(actual)) commit = actual;
    sourceState = git('status', '--porcelain', '--untracked-files=normal') ? 'modified' : 'clean';
  } catch {
    // A source archive cannot prove that its files match the injected commit.
    if (commit === 'unknown') {
      const injected = (env.TOKEN_STATION_COMMIT_HASH || env.GITHUB_SHA || '').trim();
      if (/^[0-9a-f]{7,40}$/i.test(injected)) commit = injected.slice(0, 7);
    }
  }
  const requested = env.TOKEN_STATION_BUILD_CHANNEL || (command === 'serve' ? 'development' : 'local');
  const channel = ['local', 'development', 'preview', 'production'].includes(requested) ? requested : 'unknown';
  return { commit, sourceState, channel, builtAt: new Date().toISOString() };
}
