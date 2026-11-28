import type { AppData } from "./account/core/service";

// What the relay holds for an account: its quota object and the groups and push tokens under it.
export const relayData = (env: Cloudflare.Env): AppData => {
  const quota = (account: string) => env.QUOTAS.get(env.QUOTAS.idFromName(account));
  return {
    erase: (account) => quota(account).erase(),
    export: (account) => quota(account).export(),
  };
};
