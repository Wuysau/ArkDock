import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, writeFileSync, copyFileSync, chmodSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const moduleDirectory = dirname(fileURLToPath(import.meta.url));
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
function git(cwd, args) {
  const result = spawnSync('git', ['-C', cwd, ...args], { encoding: 'utf8', windowsHide: true, shell: false });
  if (result.status !== 0 || result.error) throw new Error(result.stderr?.trim() || 'Git unavailable');
  return result.stdout.trim();
}
const psLiteral = value => `'${value.replaceAll("'", "''")}'`;
const shLiteral = value => `'${value.replaceAll("'", "'\\''")}'`;
function safeDirectory(path) {
  if (existsSync(path) && lstatSync(path).isSymbolicLink()) throw new Error(`Refusing linked hook directory: ${path}`);
  mkdirSync(path, { recursive: true });
}
function writeChanged(path, content) {
  if (existsSync(path)) {
    if (lstatSync(path).isSymbolicLink()) throw new Error('Refusing linked hook file');
    if (readFileSync(path, 'utf8') === content) return;
  }
  writeFileSync(path, content);
}

export function installTaskHooks({ cwd = process.cwd(), currentOnly = false } = {}) {
  const root = git(cwd, ['rev-parse', '--show-toplevel']);
  const common = git(cwd, ['rev-parse', '--path-format=absolute', '--git-common-dir']);
  const stable = join(common, 'arkdock-task-hooks');
  safeDirectory(stable);
  // Keep the generated local policy out of older checkouts with older ignores.
  const infoDirectory = join(common, 'info');
  safeDirectory(infoDirectory);
  const excludes = join(infoDirectory, 'exclude');
  if (existsSync(excludes) && lstatSync(excludes).isSymbolicLink()) throw new Error('Refusing linked local exclude file');
  let excludeText = existsSync(excludes) ? readFileSync(excludes, 'utf8') : '';
  for (const pattern of ['/.codex/hooks.json', '/AGENTS.md']) {
    if (!excludeText.split(/\r?\n/).includes(pattern)) excludeText = `${excludeText.trimEnd()}\n${pattern}\n`;
  }
  writeChanged(excludes, excludeText);
  const installed = join(stable, 'installation.json');
  let metadata;
  if (currentOnly) {
    if (!existsSync(installed)) throw new Error('Install the task hook before bootstrapping another worktree');
    metadata = JSON.parse(readFileSync(installed, 'utf8'));
    if (resolve(metadata.common) !== resolve(common)) throw new Error('Repository identity changed');
  } else {
    // Runtime paths remain stable when the completed task checkout is archived.
    const runtimeSource = join(moduleDirectory, 'task-completion.mjs');
    const bytes = readFileSync(runtimeSource);
    const digest = hash(bytes);
    const versionDirectory = join(stable, digest);
    safeDirectory(versionDirectory);
    const runtime = join(versionDirectory, 'task-completion.mjs');
    if (existsSync(runtime) && hash(readFileSync(runtime)) !== digest) throw new Error('Pinned hook runtime differs');
    if (!existsSync(runtime)) copyFileSync(runtimeSource, runtime);
    const installer = join(stable, 'install-task-hooks.mjs');
    writeChanged(installer, readFileSync(fileURLToPath(import.meta.url), 'utf8'));
    const launcher = join(versionDirectory, 'stop.ps1');
    const repo = git(root, ['worktree', 'list', '--porcelain']).split('\n').find(line => line.startsWith('worktree '))?.slice(9);
    if (!repo) throw new Error('Stable main repository root unavailable');
    writeChanged(launcher, `$ErrorActionPreference = 'Stop'\n$eventJson = [Console]::In.ReadToEnd()\n$actualHash = (Get-FileHash -LiteralPath ${psLiteral(runtime)} -Algorithm SHA256).Hash.ToLowerInvariant()\nif ($actualHash -ne '${digest}') { throw 'Pinned ArkDock hook runtime was modified' }\n$eventJson | & ${psLiteral(process.execPath)} ${psLiteral(runtime)} 'stop' '--repo' ${psLiteral(repo)}\nexit $LASTEXITCODE\n`);
    const command = `pwsh -NoProfile -File "${launcher.replaceAll('\\', '/')}"`;
    metadata = { version: 1, common, node: process.execPath, runtime, digest, launcher, installer, repo, command };
    writeChanged(installed, `${JSON.stringify(metadata, null, 2)}\n`);
  }
  if (hash(readFileSync(metadata.runtime)) !== metadata.digest) throw new Error('Installed task runtime integrity failed');
  const worktrees = currentOnly ? [root] : git(root, ['worktree', 'list', '--porcelain']).split('\n')
    .filter(line => line.startsWith('worktree ')).map(line => line.slice(9));
  const configurations = [];
  for (const worktree of worktrees) {
    if (!existsSync(worktree)) continue;
    if (git(worktree, ['rev-parse', '--path-format=absolute', '--git-common-dir']) !== common) throw new Error('Different repository');
    const directory = join(worktree, '.codex');
    safeDirectory(directory);
    const path = join(directory, 'hooks.json');
    if (existsSync(path) && lstatSync(path).isSymbolicLink()) throw new Error('Refusing linked Codex hook config');
    const config = existsSync(path) ? JSON.parse(readFileSync(path, 'utf8')) : {};
    const groups = config.hooks?.Stop ?? [];
    const retained = groups.map(group => ({ ...group, hooks: (group.hooks ?? []).filter(handler => !handler.statusMessage?.startsWith('ArkDock task completion')) }))
      .filter(group => group.hooks.length);
    const definition = { type: 'command', command: metadata.command, commandWindows: metadata.command, timeout: 180,
      statusMessage: 'ArkDock task completion: commit, integrate dev, archive worktree' };
    config.hooks = { ...config.hooks, Stop: [...retained, { hooks: [definition] }] };
    config.description ??= 'Project-local ArkDock lifecycle hooks';
    writeChanged(path, `${JSON.stringify(config, null, 2)}\n`);
    // This is local workflow policy, never a tracked public Harness file.
    const instructions = join(worktree, 'AGENTS.md');
    if (existsSync(instructions) && lstatSync(instructions).isSymbolicLink()) throw new Error('Refusing linked project instructions');
    const start = '<!-- arkdock-task-completion -->';
    const end = '<!-- /arkdock-task-completion -->';
    let text = existsSync(instructions) ? readFileSync(instructions, 'utf8') : '# ArkDock local task workflow\n';
    const lifecycle = `& ${psLiteral(metadata.node)} ${psLiteral(metadata.runtime)}`;
    const block = `${start}\n协作流程：Worker Agent → Integration Agent → 用户验收，详见 docs/development/agent-workflow.md。实现任务从最新 dev 创建干净独立工作树，先执行生命周期工具 begin 登记当前 CODEX_THREAD_ID；Worker 只开发、验证和提交本任务，报告分支、完整 SHA、范围及测试结果，不直接修改 dev/main，不执行 ready/finish。PowerShell 生命周期工具为 \`${lifecycle}\`。ready --complete --message "提交说明" 与 finish/Stop 自动快进仅由 Integration Agent 在任务实际完成、用户明确授权、代码已审阅且完整集成验证通过后使用；工具会提交、合入 dev 并归档，不得替代用户验收后的清理阶段。main 仅用于稳定发布；长期仅保留原始 ArkDock 目录检出 dev。状态问答、暂停、未完成或等待验收不能标记 ready。dev 有他人修改、冲突、未保存成果或进程占用时保留现场，不强删、不推送。测试遵守用户本轮约定，NOT_RUN 不能当 PASS。新 hook 定义需要用户在 /hooks 信任一次。\n${end}`;
    const offset = text.indexOf(start), tail = text.indexOf(end);
    if (offset >= 0 && tail >= offset) text = `${text.slice(0, offset)}${block}${text.slice(tail + end.length)}`;
    else text = `${text.trimEnd()}\n\n${block}\n`;
    writeChanged(instructions, text);
    configurations.push(path);
  }
  // Git's repository-local checkout hook bootstraps future linked checkouts.
  // Existing active hooks are never replaced or hidden.
  let bootstrap = false;
  if (!currentOnly) {
    const hooksDirectory = join(stable, 'git');
    // Respect effective settings inherited from global/system/includes too.
    const configured = spawnSync('git', ['-C', root, 'config', '--path', '--get', 'core.hooksPath'], { encoding: 'utf8', shell: false, windowsHide: true });
    if (configured.error || ![0, 1].includes(configured.status)) throw new Error('Cannot inspect effective Git hooks configuration');
    const existingPath = configured.stdout?.trim();
    const defaultHooks = join(common, 'hooks');
    const activeDefault = existsSync(defaultHooks) && readdirSync(defaultHooks).some(name => !name.endsWith('.sample'));
    if ((existingPath && resolve(existingPath) !== resolve(hooksDirectory)) || (!existingPath && activeDefault)) {
      console.error('Existing Git hooks retained; run install-task-hooks.mjs --current in newly created checkouts.');
    } else {
      safeDirectory(hooksDirectory);
      const checkout = join(hooksDirectory, 'post-checkout');
      writeChanged(checkout, `#!/bin/sh\nexec ${shLiteral(metadata.node)} ${shLiteral(metadata.installer)} --current\n`);
      chmodSync(checkout, 0o755);
      git(root, ['config', '--local', 'core.hooksPath', hooksDirectory]);
      bootstrap = true;
    }
  }
  return { configurations, nativeTrust: 'REQUIRED: review this exact definition in Codex /hooks', bootstrap,
    runtime: metadata.runtime, runtimeSha256: metadata.digest, common, instructions: 'begin on clean task tree; ready --complete only after real task completion' };
}

if (process.argv[1] && import.meta.url.toLowerCase() === pathToFileURL(resolve(process.argv[1])).href.toLowerCase()) {
  try {
    const options = process.argv.slice(2);
    if (options.some(option => option !== '--current')) throw new Error('Only --current is supported');
    console.log(JSON.stringify(installTaskHooks({ currentOnly: options.includes('--current') }), null, 2));
  } catch (error) { console.error(`BLOCKED task-hook installation: ${error.message}`); process.exitCode = 2; }
}
