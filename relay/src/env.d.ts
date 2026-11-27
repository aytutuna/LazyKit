declare namespace Cloudflare {
  interface GlobalProps {
    mainModule: typeof import("./index");
  }
  interface Env {
    ROOMS: DurableObjectNamespace<import("./room").Room>;
    GROUPS: DurableObjectNamespace<import("./group").Group>;
    PURCHASES: DurableObjectNamespace<import("./purchase").Purchase>;
    INSTALLS: DurableObjectNamespace<import("./install").Install>;
    APPLE_APP_IDS?: string;
    APPLE_ATTEST_ENV?: string;
    PLAY_PACKAGES?: string;
  }
}
