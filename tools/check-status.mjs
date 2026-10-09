// Only ArkDock's own gate commands reserve exit 2 for BLOCKED.
export function exitStatus(exitCode, errorCode, blockedExitCodes = []) {
  if (errorCode) return 'BLOCKED';
  if (exitCode === 0) return 'PASS';
  return blockedExitCodes.includes(exitCode) ? 'BLOCKED' : 'FAIL';
}
