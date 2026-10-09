import { readFileSync } from 'node:fs';
import { sourceFiles } from './files.mjs';

const errors = [];
for (const path of sourceFiles()) {
  if (!/\.(md|mjs|ts|tsx|css|json|yaml|yml|toml|rs|ets|html|ps1|svg)$/.test(path)) continue;
  const text = readFileSync(path, 'utf8');
  if (text.includes('\r')) errors.push(`${path}: 使用 LF`);
  if (!text.endsWith('\n')) errors.push(`${path}: 缺少末尾换行`);
  if (text.split('\n').some((line) => /[ \t]+$/.test(line))) errors.push(`${path}: 行末空白`);
  if (path.endsWith('.json')) {
    try { JSON.parse(text); } catch { errors.push(`${path}: 无效 JSON`); }
  }
}
for (const error of errors) console.error(`FAIL ${error}`);
if (errors.length) process.exitCode = 1;
else console.log('PASS 仓库格式（LF/末尾换行/行末空白/JSON）；Rust 格式由 cargo fmt 单独检查。');
