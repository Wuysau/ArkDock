import { readdirSync } from 'node:fs';
import { join } from 'node:path';
import { findForbiddenPublicPaths } from './check-public-tree.mjs';

const excluded = new Set(['.git', '.agents', '.codex', '.aws', '.local', '.playwright-cli', 'node_modules', 'dist', 'target', 'artifacts']);
export function sourceFiles(root = '.') {
  const paths = [];
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (excluded.has(entry.name) || entry.isSymbolicLink()) continue;
    const path = join(root, entry.name).replaceAll('\\', '/');
    if (findForbiddenPublicPaths([path]).length) continue;
    if (/^examples\/native-counter\/(?:entry\/)?(?:build|\.hvigor|\.preview|oh_modules)$/.test(path)) continue;
    // tauri-build writes vendor schemas without our source-format conventions.
    if (path === 'src-tauri/gen/schemas') continue;
    if (entry.isDirectory()) paths.push(...sourceFiles(path));
    else paths.push(path);
  }
  return paths;
}
