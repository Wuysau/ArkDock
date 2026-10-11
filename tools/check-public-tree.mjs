import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const forbidden = [
  /(?:^|\/)(?:artifacts|\.local|\.codex|\.agents|\.playwright-(?:cli|mcp))(?:\/|$)/i,
  /(?:^|\/)(?:AGENTS|CLAUDE)\.md$/i,
  /(?:^|\/)ENVIRONMENT\.md$/i,
  /^docs\/(?:harness|plans|evidence)(?:\/|$)/i,
  /^docs\/status\.md$/i,
  /^docs\/design\/agent-selection-delivery\.md$/i,
  /^docs\/native-preview\/(?:design|plan|integration|validation|inspector-validation|windows-setup)\.md$/i,
  /^docs\/runbooks\/windows-native-preview\.md$/i,
  /^tools\/verify-harness\.mjs$/i,
];

export function findForbiddenPublicPaths(paths) {
  return paths.map((path) => path.replaceAll('\\', '/')).filter((path) => forbidden.some((pattern) => pattern.test(path)));
}

function main() {
  const result = spawnSync('git', ['ls-files', '-z'], { encoding: 'utf8', shell: false, windowsHide: true });
  if (result.error || result.status !== 0) {
    console.error(`BLOCKED public-tree: git ls-files unavailable${result.error ? ` (${result.error.code})` : ''}`);
    process.exitCode = 2;
    return;
  }
  const tracked = result.stdout.split('\0').filter(Boolean);
  const violations = findForbiddenPublicPaths(tracked);
  if (violations.length) {
    for (const path of violations) console.error(`FAIL forbidden tracked path: ${path}`);
    process.exitCode = 1;
    return;
  }
  console.log(`PASS public-tree: ${tracked.length} tracked paths; no private Harness paths`);
}

if (process.argv[1] && import.meta.url.toLowerCase() === pathToFileURL(resolve(process.argv[1])).href.toLowerCase()) main();
