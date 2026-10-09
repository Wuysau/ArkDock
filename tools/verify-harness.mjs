import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { sourceFiles } from './files.mjs';

const errors = [];
const required = ['AGENTS.md', 'CLAUDE.md', 'README.md', 'docs/index.md', 'docs/product.md', 'docs/architecture.md', 'docs/status.md', 'docs/harness/workflow.md', 'docs/harness/quality.md', 'docs/harness/security.md', 'docs/runbooks/windows-native-preview.md', 'docs/evidence/compatibility.md', 'docs/evidence/sources.md', 'pnpm-lock.yaml'];
for (const path of required) if (!existsSync(path)) errors.push(`入口/文档缺失 ${path}`);
const agents = readFileSync('AGENTS.md', 'utf8');
if (agents.trimEnd().split('\n').length > 120) errors.push('AGENTS.md 超过 120 行');
if (agents.split('\n').some((line) => line.length > 160)) errors.push('AGENTS.md 超长行');
const claude = readFileSync('CLAUDE.md', 'utf8');
if (claude.length > 1000 || /^\s*@/m.test(claude)) errors.push('CLAUDE.md 必须为短入口，不递归导入');
if (/必须.*每次.*superpowers|每次.*必须.*skills|load all docs|读取所有文档/i.test(agents)) errors.push('入口含全量加载风险');
for (const path of sourceFiles().filter((p) => p.endsWith('.md'))) {
  const text = readFileSync(path, 'utf8');
  for (const match of text.matchAll(/\[[^\]]+\]\(([^)]+)\)/g)) {
    const link = match[1].split('#')[0];
    if (!link || /^(https?:|mailto:|app:)/.test(link)) continue;
    if (!existsSync(resolve(dirname(path), link))) errors.push(`${path}: 断链 ${link}`);
  }
}
for (const match of agents.matchAll(/`((?:docs|tools|src|protocol)\/[^`]+\.(?:md|mjs|json|rs|ts))`/g)) {
  if (!existsSync(match[1])) errors.push(`AGENTS 引用缺失 ${match[1]}`);
}
const pkg = JSON.parse(readFileSync('package.json', 'utf8'));
for (const command of ['verify', 'verify:native', 'verify:release', 'doctor', 'typecheck', 'lint', 'rust:fmt', 'rust:clippy', 'rust:test']) if (!pkg.scripts[command]) errors.push(`未接通命令 ${command}`);
for (const [name, command] of Object.entries(pkg.scripts)) {
  for (const match of command.matchAll(/tools\/[a-z-]+\.mjs/g)) if (!existsSync(match[0])) errors.push(`${name}: 缺少脚本 ${match[0]}`);
}
if (!existsSync('.github/workflows/quality.yml')) errors.push('缺少实际 CI');
for (const error of errors) console.error(`FAIL ${error}`);
if (errors.length) process.exitCode = 1;
else console.log('PASS Harness 入口/引用/命令/CI 结构；不证明所有客户端实际加载行为。');
