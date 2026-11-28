import type { AppData } from "./account/api";

const quota = (env: Cloudflare.Env, account: string) => env.QUOTAS.get(env.QUOTAS.idFromName(account));

// What the relay holds for an account: its quota object and the groups and push tokens under it.
export const relayData: AppData = {
  erase: (env, account) => quota(env, account).erase(),
  export: (env, account) => quota(env, account).export(),
};
