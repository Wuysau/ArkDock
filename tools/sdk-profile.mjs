import { readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';

export const profile = JSON.parse(readFileSync(new URL('../protocol/native-profile.json',import.meta.url),'utf8'));
export function matchSdk(root) {
  try {
    const manifestPath = join(root,'sdk/default/sdk-pkg.json');
    if (statSync(manifestPath).size > 65536) return null;
    const sdk = JSON.parse(readFileSync(manifestPath,'utf8')).data;
    if (!sdk) return null;
    if (sdk.version !== profile.sdkVersion || Number(sdk.apiVersion) !== profile.apiVersion) return null;
    for (const [relative, expected] of Object.entries(profile.files)) {
      const path = join(root,relative);
      if (statSync(path).size > 16 * 1024 * 1024 || createHash('sha256').update(readFileSync(path)).digest('hex') !== expected) return null;
    }
    return profile;
  } catch { return null; }
}
