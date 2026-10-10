// Test drivers must never select the first unrelated ArkDock process by name.
import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';

export function ownedDesktop() {
  const requestedPid = process.env.ARKDOCK_DESKTOP_PID;
  if (requestedPid && !/^[1-9]\d{0,9}$/.test(requestedPid)) throw new Error('Invalid ARKDOCK_DESKTOP_PID');
  const expected = process.env.ARKDOCK_DESKTOP_EXE
    ? [resolve(process.env.ARKDOCK_DESKTOP_EXE)]
    : ['debug', 'release'].map(mode => resolve(process.env.CARGO_TARGET_DIR ?? 'src-tauri/target', mode, 'arkdock.exe'));
  const normalize = value => String(value).replaceAll('\\', '/').toLowerCase();
  const records = JSON.parse(execFileSync('pwsh', ['-NoProfile', '-Command',
    'ConvertTo-Json -InputObject @(Get-CimInstance Win32_Process -Filter "name=\'arkdock.exe\'" | Select-Object ProcessId,ExecutablePath) -Compress'],
  { encoding: 'utf8', windowsHide: true, timeout: 12_000 }));
  const matches = records.filter(item => (!requestedPid || item.ProcessId === Number(requestedPid))
    && expected.some(path => normalize(path) === normalize(item.ExecutablePath)));
  if (matches.length !== 1) throw new Error('BLOCKED: start one ArkDock instance owned by this worktree; set ARKDOCK_DESKTOP_PID/EXE explicitly for another authorized test build.');
  return { Id: matches[0].ProcessId, executable: matches[0].ExecutablePath };
}
