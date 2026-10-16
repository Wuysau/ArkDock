import { useEffect, useMemo, useState } from 'react';
import type { UiBridge } from '../ipc/client.ts';
import type { McpStatus } from '../ipc/model.ts';
import { describeError } from '../ipc/errors.ts';
import { createStatusPoller } from './model.ts';

// Switching from the inspector to the setup sheet still shares one native read.
const pollers = new WeakMap<UiBridge, ReturnType<typeof createStatusPoller<McpStatus>>>();
function statusPoller(bridge: UiBridge) {
  let poller = pollers.get(bridge);
  if (!poller) {
    poller = createStatusPoller(async () => {
      if (!bridge.mcpStatus) throw new Error('当前桌面桥接不支持 MCP 接入。');
      return bridge.mcpStatus();
    });
    pollers.set(bridge, poller);
  }
  return poller;
}

export function useMcpStatus(bridge: UiBridge, lifecycle: string, visible = true) {
  const [value, setValue] = useState<{ lifecycle: string; status: McpStatus } | null>(null);
  const [failure, setFailure] = useState<{ lifecycle: string; error: string } | null>(null);
  const poller = useMemo(() => statusPoller(bridge), [bridge]);
  useEffect(() => {
    if (!visible || !bridge.available) return;
    setValue(null); setFailure(null);
    return poller.start(status => { setValue({ lifecycle, status }); setFailure(null); }, error => {
      setValue(null); setFailure({ lifecycle, error: describeError(error) });
    });
  }, [poller, bridge.available, lifecycle, visible]);
  return { status: visible && value?.lifecycle === lifecycle ? value.status : null,
    error: visible && failure?.lifecycle === lifecycle ? failure.error : '' };
}
