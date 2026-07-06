import { useEffect, useState } from 'react';

import { type DaemonStatusResult, extractData, extractError } from './daemon';

// Lazy per-view daemon fetch for methods outside the always-polled global
// snapshot (My Work wants workset.list / mount.status; admin views want
// audit.events / snapshot.list / etc.). Fires once on mount; the global
// refreshNonce can be passed to re-fetch on manual refresh.
//
// Envelope is treated as untrusted draft data, same as everywhere else.
export function useDaemonFetch(
  method: string,
  params: Record<string, unknown> = {},
  refreshNonce = 0,
): {
  data: Record<string, unknown> | null;
  error: { code: string; message: string } | null;
  loading: boolean;
} {
  const [result, setResult] = useState<DaemonStatusResult | null>(null);
  const paramsKey = JSON.stringify(params);

  useEffect(() => {
    let cancelled = false;
    void window.biohazardfs.rpc(method, params).then((r) => {
      if (!cancelled) setResult(r);
    });
    return () => {
      cancelled = true;
    };
    // paramsKey (the JSON of params) is the stable semantic dependency; listing
    // `params` directly would re-fetch every render since it's a new object.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [method, paramsKey, refreshNonce]);

  return {
    data: extractData(result),
    error: extractError(result),
    loading: result === null,
  };
}
